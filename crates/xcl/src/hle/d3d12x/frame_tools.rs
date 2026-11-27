//! Offline XCLFRAME tooling: list-op edit, content/plane pixel compare, diverge reports.
//!
//! GPL-2 stand-ins for CGBJ `victory replay --pixel` / `--diverge` / edit experiments. These
//! operate on recorded `XCLFRAME` / JSONL captures without a GPU device. Live same-process content
//! diverge is `XCL_REPLAY_DIVERGE` in [`super::frame_replay`]. Multi-run GPU picture diverge
//! (CGBJ `play::diverge_picture`) needs offline command rebuild — **impossible** across processes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::frame_wire::{self, list_op, fnv1a32, WireEvent};

/// One decoded record inside a [`WireEvent::ListPayload`] `ops` blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListOp {
    /// `D3D12_COMMAND_LIST_TYPE` from the XLST snap (`LIST_KIND`).
    ListKind(u32),
    BindIds { pso: u64, rs: u64 },
    Heaps(Vec<u64>),
    TouchVa { va: u64, len: u32 },
    Written { va: u64, end: u64 },
    Draw {
        indexed: bool,
        count: u32,
        instances: u32,
        first: u32,
        base: i32,
        /// Present on `DRAW_EX`; 0 when decoded from legacy `DRAW`.
        first_instance: u32,
    },
    Dispatch { x: u32, y: u32, z: u32 },
    Indirect {
        sig: u64,
        max: u32,
        args: u64,
        args_off: u64,
        count: u64,
        count_off: u64,
    },
    DispatchMesh { x: u32, y: u32, z: u32 },
    /// Unknown tag kept so edit round-trips do not drop future ops.
    Raw(Vec<u8>),
}

/// Decode an `XLST` + live draw/dispatch ops blob into structured [`ListOp`]s.
pub fn decode_ops(ops: &[u8]) -> Result<Vec<ListOp>, String> {
    let mut i = 0usize;
    let mut out = Vec::new();
    if ops.len() >= 5 && &ops[..4] == list_op::SNAP_MAGIC {
        let ver = ops[4];
        if ver != list_op::SNAP_VER {
            return Err(format!("XLST version {ver} unsupported (want {})", list_op::SNAP_VER));
        }
        i = 5;
    }
    while i < ops.len() {
        let tag = ops[i];
        i += 1;
        match tag {
            list_op::LIST_KIND => {
                let kind = take_u32(ops, &mut i)?;
                out.push(ListOp::ListKind(kind));
            }
            list_op::BIND_IDS => {
                let pso = take_u64(ops, &mut i)?;
                let rs = take_u64(ops, &mut i)?;
                out.push(ListOp::BindIds { pso, rs });
            }
            list_op::HEAPS => {
                let n = take_u32(ops, &mut i)? as usize;
                let mut heaps = Vec::with_capacity(n);
                for _ in 0..n {
                    heaps.push(take_u64(ops, &mut i)?);
                }
                out.push(ListOp::Heaps(heaps));
            }
            list_op::TOUCH_VA => {
                let va = take_u64(ops, &mut i)?;
                let len = take_u32(ops, &mut i)?;
                out.push(ListOp::TouchVa { va, len });
            }
            list_op::WRITTEN => {
                let va = take_u64(ops, &mut i)?;
                let end = take_u64(ops, &mut i)?;
                out.push(ListOp::Written { va, end });
            }
            list_op::DRAW | list_op::DRAW_EX => {
                let indexed = *ops.get(i).ok_or("truncated DRAW")? != 0;
                i += 1;
                let count = take_u32(ops, &mut i)?;
                let instances = take_u32(ops, &mut i)?;
                let first = take_u32(ops, &mut i)?;
                let base = take_i32(ops, &mut i)?;
                let first_instance = if tag == list_op::DRAW_EX {
                    take_u32(ops, &mut i)?
                } else {
                    0
                };
                out.push(ListOp::Draw {
                    indexed,
                    count,
                    instances,
                    first,
                    base,
                    first_instance,
                });
            }
            list_op::DISPATCH => {
                let x = take_u32(ops, &mut i)?;
                let y = take_u32(ops, &mut i)?;
                let z = take_u32(ops, &mut i)?;
                out.push(ListOp::Dispatch { x, y, z });
            }
            list_op::INDIRECT => {
                out.push(ListOp::Indirect {
                    sig: take_u64(ops, &mut i)?,
                    max: take_u32(ops, &mut i)?,
                    args: take_u64(ops, &mut i)?,
                    args_off: take_u64(ops, &mut i)?,
                    count: take_u64(ops, &mut i)?,
                    count_off: take_u64(ops, &mut i)?,
                });
            }
            list_op::DISPATCH_MESH => {
                let x = take_u32(ops, &mut i)?;
                let y = take_u32(ops, &mut i)?;
                let z = take_u32(ops, &mut i)?;
                out.push(ListOp::DispatchMesh { x, y, z });
            }
            other => {
                // Cannot size unknown tags; stash remainder so callers see the break.
                let mut raw = vec![other];
                raw.extend_from_slice(&ops[i..]);
                out.push(ListOp::Raw(raw));
                break;
            }
        }
    }
    Ok(out)
}

/// Encode [`ListOp`]s back to an `XLST` v1 ops blob (plus trailing live ops).
pub fn encode_ops(ops: &[ListOp]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(list_op::SNAP_MAGIC);
    out.push(list_op::SNAP_VER);
    for op in ops {
        match op {
            ListOp::BindIds { pso, rs } => {
                out.push(list_op::BIND_IDS);
                out.extend_from_slice(&pso.to_le_bytes());
                out.extend_from_slice(&rs.to_le_bytes());
            }
            ListOp::Heaps(heaps) => {
                out.push(list_op::HEAPS);
                out.extend_from_slice(&(heaps.len() as u32).to_le_bytes());
                for h in heaps {
                    out.extend_from_slice(&h.to_le_bytes());
                }
            }
            ListOp::TouchVa { va, len } => {
                out.push(list_op::TOUCH_VA);
                out.extend_from_slice(&va.to_le_bytes());
                out.extend_from_slice(&len.to_le_bytes());
            }
            ListOp::Written { va, end } => {
                out.push(list_op::WRITTEN);
                out.extend_from_slice(&va.to_le_bytes());
                out.extend_from_slice(&end.to_le_bytes());
            }
            ListOp::ListKind(kind) => {
                out.push(list_op::LIST_KIND);
                out.extend_from_slice(&kind.to_le_bytes());
            }
            ListOp::Draw {
                indexed,
                count,
                instances,
                first,
                base,
                first_instance,
            } => {
                if *first_instance != 0 {
                    out.push(list_op::DRAW_EX);
                    out.push(u8::from(*indexed));
                    out.extend_from_slice(&count.to_le_bytes());
                    out.extend_from_slice(&instances.to_le_bytes());
                    out.extend_from_slice(&first.to_le_bytes());
                    out.extend_from_slice(&base.to_le_bytes());
                    out.extend_from_slice(&first_instance.to_le_bytes());
                } else {
                    out.push(list_op::DRAW);
                    out.push(u8::from(*indexed));
                    out.extend_from_slice(&count.to_le_bytes());
                    out.extend_from_slice(&instances.to_le_bytes());
                    out.extend_from_slice(&first.to_le_bytes());
                    out.extend_from_slice(&base.to_le_bytes());
                }
            }
            ListOp::Dispatch { x, y, z } => {
                out.push(list_op::DISPATCH);
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
                out.extend_from_slice(&z.to_le_bytes());
            }
            ListOp::Indirect {
                sig,
                max,
                args,
                args_off,
                count,
                count_off,
            } => {
                out.push(list_op::INDIRECT);
                out.extend_from_slice(&sig.to_le_bytes());
                out.extend_from_slice(&max.to_le_bytes());
                out.extend_from_slice(&args.to_le_bytes());
                out.extend_from_slice(&args_off.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
                out.extend_from_slice(&count_off.to_le_bytes());
            }
            ListOp::DispatchMesh { x, y, z } => {
                out.push(list_op::DISPATCH_MESH);
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
                out.extend_from_slice(&z.to_le_bytes());
            }
            ListOp::Raw(raw) => out.extend_from_slice(raw),
        }
    }
    out
}

fn take_u32(ops: &[u8], i: &mut usize) -> Result<u32, String> {
    if *i + 4 > ops.len() {
        return Err("truncated u32".into());
    }
    let v = u32::from_le_bytes(ops[*i..*i + 4].try_into().unwrap());
    *i += 4;
    Ok(v)
}

fn take_i32(ops: &[u8], i: &mut usize) -> Result<i32, String> {
    if *i + 4 > ops.len() {
        return Err("truncated i32".into());
    }
    let v = i32::from_le_bytes(ops[*i..*i + 4].try_into().unwrap());
    *i += 4;
    Ok(v)
}

fn take_u64(ops: &[u8], i: &mut usize) -> Result<u64, String> {
    if *i + 8 > ops.len() {
        return Err("truncated u64".into());
    }
    let v = u64::from_le_bytes(ops[*i..*i + 8].try_into().unwrap());
    *i += 8;
    Ok(v)
}

/// Edit switches for list-op transforms (CGBJ `edit` / replay experiment role).
#[derive(Clone, Debug, Default)]
pub struct EditOpts {
    pub skip_draws: bool,
    pub skip_dispatches: bool,
    pub meta_only: bool,
    /// Keep only lists whose ordinal (0-based among ListPayload events) is in `lo..=hi`.
    pub only_lists: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Default)]
pub struct EditReport {
    pub lists_seen: u32,
    pub lists_kept: u32,
    pub draws_dropped: u32,
    pub dispatches_dropped: u32,
    pub ops_in: usize,
    pub ops_out: usize,
}

/// Apply [`EditOpts`] to a list's decoded ops.
pub fn edit_ops(ops: &[ListOp], opts: &EditOpts) -> (Vec<ListOp>, EditReport) {
    let mut report = EditReport {
        lists_seen: 1,
        lists_kept: 1,
        ops_in: ops.len(),
        ..Default::default()
    };
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            ListOp::Draw { .. } if opts.skip_draws || opts.meta_only => {
                report.draws_dropped += 1;
            }
            ListOp::Dispatch { .. } | ListOp::DispatchMesh { .. } | ListOp::Indirect { .. }
                if opts.skip_dispatches || opts.meta_only =>
            {
                report.dispatches_dropped += 1;
            }
            other => out.push(other.clone()),
        }
    }
    report.ops_out = out.len();
    (out, report)
}

fn merge_report(into: &mut EditReport, part: &EditReport) {
    into.lists_seen += part.lists_seen;
    into.lists_kept += part.lists_kept;
    into.draws_dropped += part.draws_dropped;
    into.dispatches_dropped += part.dispatches_dropped;
    into.ops_in += part.ops_in;
    into.ops_out += part.ops_out;
}

/// Load every `frames/frame_*.bin` (preferred) or `.jsonl` under `dir`.
fn load_frame_events(dir: &Path, frame: u64) -> Result<Vec<WireEvent>, String> {
    let bin = dir.join("frames").join(format!("frame_{frame:04}.bin"));
    if bin.is_file() {
        return frame_wire::read_frame(&bin);
    }
    Err(format!("missing {}", bin.display()))
}

fn frame_count(dir: &Path) -> u64 {
    let mut n = 0u64;
    loop {
        n += 1;
        let bin = dir.join("frames").join(format!("frame_{n:04}.bin"));
        let jsonl = dir.join("frames").join(format!("frame_{n:04}.jsonl"));
        if !bin.is_file() && !jsonl.is_file() {
            return n.saturating_sub(1);
        }
    }
}

/// Dump decoded list ops for one capture (`--listing`).
pub fn cli_listing(dir: &Path, frame_filter: Option<u64>, json: bool) -> Result<String, String> {
    let n_frames = frame_count(dir);
    if n_frames == 0 {
        return Err("no frames found".into());
    }
    let mut rows = Vec::new();
    for f in 1..=n_frames {
        if frame_filter.is_some_and(|want| want != f) {
            continue;
        }
        let events = load_frame_events(dir, f)?;
        let mut list_i = 0u32;
        for ev in &events {
            let WireEvent::ListPayload { list, kind, ops } = ev else {
                continue;
            };
            let decoded = decode_ops(ops).unwrap_or_default();
            let summary: Vec<String> = decoded
                .iter()
                .map(|op| match op {
                    ListOp::ListKind(k) => format!("list_kind {k}"),
                    ListOp::BindIds { pso, rs } => format!("bind pso={pso:#x} rs={rs:#x}"),
                    ListOp::Heaps(h) => format!("heaps {}", h.len()),
                    ListOp::TouchVa { va, len } => format!("touch {va:#x}+{len}"),
                    ListOp::Written { va, end } => format!("written {va:#x}..{end:#x}"),
                    ListOp::Draw {
                        indexed,
                        count,
                        instances,
                        first_instance,
                        ..
                    } => format!(
                        "draw{} n={count} inst={instances} first_inst={first_instance}",
                        if *indexed { "i" } else { "" }
                    ),
                    ListOp::Dispatch { x, y, z } => format!("dispatch {x}x{y}x{z}"),
                    ListOp::Indirect { max, .. } => format!("indirect max={max}"),
                    ListOp::DispatchMesh { x, y, z } => format!("dispatch_mesh {x}x{y}x{z}"),
                    ListOp::Raw(r) => format!("raw {} B", r.len()),
                })
                .collect();
            rows.push(serde_json::json!({
                "frame": f,
                "list_ord": list_i,
                "list": list,
                "kind": kind,
                "ops": summary,
            }));
            list_i += 1;
        }
    }
    if json {
        Ok(serde_json::json!({ "dir": dir.display().to_string(), "lists": rows }).to_string())
    } else {
        let mut out = format!("xcl replay listing {}\n", dir.display());
        for r in &rows {
            out.push_str(&format!(
                "  frame {} list#{} ptr={:#x} kind={}: {}\n",
                r["frame"],
                r["list_ord"],
                r["list"].as_u64().unwrap_or(0),
                r["kind"],
                r["ops"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("; "))
                    .unwrap_or_default()
            ));
        }
        Ok(out)
    }
}

/// Apply edits across all wire frames; optionally write rewritten bins under `out_dir`.
pub fn cli_edit(
    dir: &Path,
    opts: &EditOpts,
    out_dir: Option<&Path>,
    frame_filter: Option<u64>,
    json: bool,
) -> Result<String, String> {
    let n_frames = frame_count(dir);
    if n_frames == 0 {
        return Err("no frames found".into());
    }
    let mut total = EditReport::default();
    let mut frame_rows = Vec::new();
    for f in 1..=n_frames {
        if frame_filter.is_some_and(|want| want != f) {
            continue;
        }
        let events = load_frame_events(dir, f)?;
        let mut list_ord = 0u32;
        let mut new_events = Vec::with_capacity(events.len());
        let mut frame_rep = EditReport::default();
        for ev in events {
            match ev {
                WireEvent::ListPayload { list, kind, ops } => {
                    let keep = opts
                        .only_lists
                        .map(|(lo, hi)| (lo..=hi).contains(&list_ord))
                        .unwrap_or(true);
                    list_ord += 1;
                    if !keep {
                        frame_rep.lists_seen += 1;
                        // Drop payload when filtering lists out of the rewrite.
                        if out_dir.is_some() {
                            continue;
                        }
                        new_events.push(WireEvent::ListPayload { list, kind, ops });
                        continue;
                    }
                    let decoded = decode_ops(&ops)?;
                    let (edited, part) = edit_ops(&decoded, opts);
                    merge_report(&mut frame_rep, &part);
                    new_events.push(WireEvent::ListPayload {
                        list,
                        kind,
                        ops: encode_ops(&edited),
                    });
                }
                other => new_events.push(other),
            }
        }
        merge_report(&mut total, &frame_rep);
        if let Some(out) = out_dir {
            let frames = out.join("frames");
            std::fs::create_dir_all(&frames).map_err(|e| e.to_string())?;
            let path = frames.join(format!("frame_{f:04}.bin"));
            frame_wire::write_frame(&path, &new_events).map_err(|e| e.to_string())?;
        }
        frame_rows.push(serde_json::json!({
            "frame": f,
            "lists_seen": frame_rep.lists_seen,
            "lists_kept": frame_rep.lists_kept,
            "draws_dropped": frame_rep.draws_dropped,
            "dispatches_dropped": frame_rep.dispatches_dropped,
            "ops_in": frame_rep.ops_in,
            "ops_out": frame_rep.ops_out,
        }));
    }
    if json {
        Ok(serde_json::json!({
            "dir": dir.display().to_string(),
            "out": out_dir.map(|p| p.display().to_string()),
            "edit": {
                "skip_draws": opts.skip_draws,
                "skip_dispatches": opts.skip_dispatches,
                "meta_only": opts.meta_only,
                "only_lists": opts.only_lists,
            },
            "totals": {
                "lists_seen": total.lists_seen,
                "lists_kept": total.lists_kept,
                "draws_dropped": total.draws_dropped,
                "dispatches_dropped": total.dispatches_dropped,
                "ops_in": total.ops_in,
                "ops_out": total.ops_out,
            },
            "frames": frame_rows,
            "note": "Offline ListPayload rewrite only; same-process ExecuteCommandLists still uses live COM lists unless a title re-records.",
        })
        .to_string())
    } else {
        let mut out = format!(
            "xcl replay edit {}\ndropped {} draw(s), {} dispatch(es); ops {} → {}\n",
            dir.display(),
            total.draws_dropped,
            total.dispatches_dropped,
            total.ops_in,
            total.ops_out
        );
        if let Some(p) = out_dir {
            out.push_str(&format!("wrote rewritten wire under {}\n", p.display()));
        }
        for r in &frame_rows {
            out.push_str(&format!(
                "  frame {}: draws_dropped={} dispatches_dropped={} ops {}→{}\n",
                r["frame"], r["draws_dropped"], r["dispatches_dropped"], r["ops_in"], r["ops_out"]
            ));
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
struct ContentKey {
    frame: u64,
    va: u64,
    len: u32,
    hash: u32,
    kind: &'static str,
    file_off: Option<u64>,
}

fn collect_content(dir: &Path, frame_filter: Option<u64>) -> Result<Vec<ContentKey>, String> {
    let n = frame_count(dir);
    let mut out = Vec::new();
    for f in 1..=n {
        if frame_filter.is_some_and(|want| want != f) {
            continue;
        }
        let events = load_frame_events(dir, f)?;
        for ev in events {
            match ev {
                WireEvent::ContentSample { va, len, hash, .. } => out.push(ContentKey {
                    frame: f,
                    va,
                    len,
                    hash,
                    kind: "sample",
                    file_off: None,
                }),
                WireEvent::ContentChunk {
                    va,
                    len,
                    hash,
                    file_off,
                } => out.push(ContentKey {
                    frame: f,
                    va,
                    len,
                    hash,
                    kind: "chunk",
                    file_off: Some(file_off),
                }),
                _ => {}
            }
        }
    }
    Ok(out)
}

fn present_planes(dir: &Path, frame_filter: Option<u64>) -> Result<Vec<serde_json::Value>, String> {
    let n = frame_count(dir);
    let mut out = Vec::new();
    for f in 1..=n {
        if frame_filter.is_some_and(|want| want != f) {
            continue;
        }
        let events = load_frame_events(dir, f)?;
        for ev in events {
            if let WireEvent::Present {
                frame,
                token,
                queue,
                planes,
                ..
            } = ev
            {
                for (i, p) in planes.iter().enumerate() {
                    let (w, h) = p
                        .rect
                        .map(|r| ((r[2] - r[0]).max(0) as u32, (r[3] - r[1]).max(0) as u32))
                        .unwrap_or((0, 0));
                    out.push(serde_json::json!({
                        "frame": f,
                        "present_frame": frame,
                        "token": token,
                        "queue": queue,
                        "plane": i,
                        "resource": p.resource,
                        "rect": p.rect,
                        "width": w,
                        "height": h,
                    }));
                }
            }
        }
    }
    Ok(out)
}

/// Map a picture point into a present-plane rect (offline stand-in for CGBJ `--pixel`).
fn point_in_rect(x: u32, y: u32, picture: (u32, u32), rect: [i32; 4]) -> Option<(u32, u32)> {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let rw = (rect[2] - rect[0]).max(1) as f32;
    let rh = (rect[3] - rect[1]).max(1) as f32;
    let tx = ((x as f32 + 0.5) / pw * rw).floor() as i32 + rect[0];
    let ty = ((y as f32 + 0.5) / ph * rh).floor() as i32 + rect[1];
    if tx >= rect[0] && ty >= rect[1] && tx < rect[2] && ty < rect[3] {
        Some((tx as u32, ty as u32))
    } else {
        None
    }
}

/// Offline pixel / content compare (`--pixel` / `--compare`).
pub fn cli_pixel(
    dir: &Path,
    points: &[(u32, u32)],
    picture: Option<(u32, u32)>,
    against: Option<&Path>,
    frame_filter: Option<u64>,
    json: bool,
) -> Result<String, String> {
    let planes = present_planes(dir, frame_filter)?;
    let content = collect_content(dir, frame_filter)?;
    let picture = picture.or_else(|| {
        planes
            .iter()
            .find_map(|p| {
                let w = p["width"].as_u64()? as u32;
                let h = p["height"].as_u64()? as u32;
                (w > 0 && h > 0).then_some((w, h))
            })
    });
    let mut hits = Vec::new();
    if let Some(pic) = picture {
        for &(x, y) in points {
            for p in &planes {
                let rect = match p.get("rect") {
                    Some(serde_json::Value::Array(a)) if a.len() == 4 => {
                        let mut r = [0i32; 4];
                        for (i, v) in a.iter().enumerate() {
                            r[i] = v.as_i64().unwrap_or(0) as i32;
                        }
                        r
                    }
                    _ => continue,
                };
                if let Some((tx, ty)) = point_in_rect(x, y, pic, rect) {
                    hits.push(serde_json::json!({
                        "point": [x, y],
                        "texel": [tx, ty],
                        "frame": p["frame"],
                        "plane": p["plane"],
                        "resource": p["resource"],
                        "note": "Offline plane mapping only — no live GPU texel readback",
                    }));
                }
            }
        }
    }

    let mut compare = serde_json::Value::Null;
    if let Some(other) = against {
        let a = collect_content(dir, frame_filter)?;
        let b = collect_content(other, frame_filter)?;
        let mut map_a: BTreeMap<(u64, u64, u32), u32> = BTreeMap::new();
        for c in &a {
            map_a.insert((c.frame, c.va, c.len), c.hash);
        }
        let mut matched = 0u32;
        let mut mismatched = Vec::new();
        let mut only_b = 0u32;
        for c in &b {
            match map_a.remove(&(c.frame, c.va, c.len)) {
                Some(h) if h == c.hash => matched += 1,
                Some(h) => mismatched.push(serde_json::json!({
                    "frame": c.frame,
                    "va": c.va,
                    "len": c.len,
                    "hash_a": h,
                    "hash_b": c.hash,
                })),
                None => only_b += 1,
            }
        }
        let only_a = map_a.len() as u32;
        // Optional byte-level chunk compare when both have contents.bin.
        let mut byte_diffs = Vec::new();
        for c in &content {
            if let Some(off) = c.file_off {
                let path_a = dir.join("frames").join(format!("frame_{:04}.contents.bin", c.frame));
                let path_b = other
                    .join("frames")
                    .join(format!("frame_{:04}.contents.bin", c.frame));
                if path_a.is_file() && path_b.is_file() {
                    let ba = std::fs::read(&path_a).unwrap_or_default();
                    let bb = std::fs::read(&path_b).unwrap_or_default();
                    let end = (off as usize).saturating_add(c.len as usize);
                    if end <= ba.len() && end <= bb.len() {
                        let sa = &ba[off as usize..end];
                        let sb = &bb[off as usize..end];
                        if sa != sb {
                            let mut first = None;
                            for (i, (x, y)) in sa.iter().zip(sb.iter()).enumerate() {
                                if x != y {
                                    first = Some(i);
                                    break;
                                }
                            }
                            byte_diffs.push(serde_json::json!({
                                "frame": c.frame,
                                "va": c.va,
                                "first_diff": first,
                                "hash_a": fnv1a32(sa),
                                "hash_b": fnv1a32(sb),
                            }));
                        }
                    }
                }
            }
        }
        compare = serde_json::json!({
            "against": other.display().to_string(),
            "matched": matched,
            "mismatched": mismatched,
            "only_a": only_a,
            "only_b": only_b,
            "byte_diffs": byte_diffs,
        });
    }

    if json {
        Ok(serde_json::json!({
            "dir": dir.display().to_string(),
            "picture": picture,
            "points": points,
            "plane_hits": hits,
            "present_planes": planes,
            "content_keys": content.len(),
            "compare": compare,
            "cross_process_gpu_pixel": "unsupported",
        })
        .to_string())
    } else {
        let mut out = format!("xcl replay pixel {}\n", dir.display());
        if let Some(pic) = picture {
            out.push_str(&format!("picture {}x{}\n", pic.0, pic.1));
        } else {
            out.push_str("picture size unknown (no present rects)\n");
        }
        out.push_str(&format!(
            "{} present plane(s), {} content key(s), {} point→plane hit(s)\n",
            planes.len(),
            content.len(),
            hits.len()
        ));
        for h in &hits {
            out.push_str(&format!(
                "  point ({},{}) → plane {} res={:#x} texel ({},{})\n",
                h["point"][0],
                h["point"][1],
                h["plane"],
                h["resource"].as_u64().unwrap_or(0),
                h["texel"][0],
                h["texel"][1]
            ));
        }
        if !compare.is_null() {
            out.push_str(&format!(
                "compare vs {}: matched={} mismatched={} only_a={} only_b={} byte_diffs={}\n",
                compare["against"],
                compare["matched"],
                compare["mismatched"].as_array().map(|a| a.len()).unwrap_or(0),
                compare["only_a"],
                compare["only_b"],
                compare["byte_diffs"].as_array().map(|a| a.len()).unwrap_or(0)
            ));
        }
        let present = dir.join("present.bin");
        if present.is_file() {
            let n = std::fs::metadata(&present).map(|m| m.len()).unwrap_or(0);
            out.push_str(&format!("present.bin: {n} B (session sidecar)\n"));
        }
        out.push_str("note: offline plane/content/present.bin compare; live GPU texel probe same-process only\n");
        Ok(out)
    }
}

/// Offline diverge: first content-hash / written-range split between captures or within one.
pub fn cli_diverge(
    dir: &Path,
    against: Option<&Path>,
    frame_filter: Option<u64>,
    write_report: bool,
    json: bool,
) -> Result<String, String> {
    let n = frame_count(dir);
    if n == 0 {
        return Err("no frames found".into());
    }

    let mut findings = Vec::new();

    // Intra-capture: overlapping WRITTEN ranges across lists (nondeterminism candidates).
    for f in 1..=n {
        if frame_filter.is_some_and(|want| want != f) {
            continue;
        }
        let events = load_frame_events(dir, f)?;
        let mut ranges: Vec<(u32, u64, u64, usize)> = Vec::new();
        let mut list_ord = 0u32;
        for ev in &events {
            if let WireEvent::ListPayload { list, ops, .. } = ev {
                if let Ok(decoded) = decode_ops(ops) {
                    for op in decoded {
                        if let ListOp::Written { va, end } = op {
                            ranges.push((list_ord, va, end, *list));
                        }
                    }
                }
                list_ord += 1;
            }
        }
        for i in 0..ranges.len() {
            for j in (i + 1)..ranges.len() {
                let (la, va, ea, _) = ranges[i];
                let (lb, vb, eb, _) = ranges[j];
                if la == lb {
                    continue;
                }
                if va < eb && vb < ea {
                    findings.push(serde_json::json!({
                        "kind": "written_overlap",
                        "frame": f,
                        "list_a": la,
                        "list_b": lb,
                        "va_a": va,
                        "end_a": ea,
                        "va_b": vb,
                        "end_b": eb,
                    }));
                }
            }
        }
    }

    let mut first_content_split = serde_json::Value::Null;
    if let Some(other) = against {
        let a = collect_content(dir, frame_filter)?;
        let b = collect_content(other, frame_filter)?;
        let mut map_b: BTreeMap<(u64, u64, u32), u32> = BTreeMap::new();
        for c in &b {
            map_b.insert((c.frame, c.va, c.len), c.hash);
        }
        for c in &a {
            if let Some(&hb) = map_b.get(&(c.frame, c.va, c.len)) {
                if hb != c.hash {
                    first_content_split = serde_json::json!({
                        "kind": "content_hash",
                        "frame": c.frame,
                        "va": c.va,
                        "len": c.len,
                        "hash_a": c.hash,
                        "hash_b": hb,
                        "sample_kind": c.kind,
                    });
                    findings.insert(0, first_content_split.clone());
                    break;
                }
            }
        }
        // List-op draw/dispatch count diverge.
        for f in 1..=n {
            if frame_filter.is_some_and(|want| want != f) {
                continue;
            }
            let ea = load_frame_events(dir, f)?;
            let eb = match load_frame_events(other, f) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let count_ops = |events: &[WireEvent]| -> BTreeMap<u32, (u32, u32)> {
                let mut m = BTreeMap::new();
                let mut ord = 0u32;
                for ev in events {
                    if let WireEvent::ListPayload { ops, .. } = ev {
                        let mut draws = 0u32;
                        let mut disp = 0u32;
                        if let Ok(decoded) = decode_ops(ops) {
                            for op in decoded {
                                match op {
                                    ListOp::Draw { .. } => draws += 1,
                                    ListOp::Dispatch { .. }
                                    | ListOp::DispatchMesh { .. }
                                    | ListOp::Indirect { .. } => disp += 1,
                                    _ => {}
                                }
                            }
                        }
                        m.insert(ord, (draws, disp));
                        ord += 1;
                    }
                }
                m
            };
            let ca = count_ops(&ea);
            let cb = count_ops(&eb);
            let keys: BTreeSet<u32> = ca.keys().chain(cb.keys()).copied().collect();
            for k in keys {
                let a = ca.get(&k).copied().unwrap_or((0, 0));
                let b = cb.get(&k).copied().unwrap_or((0, 0));
                if a != b {
                    findings.push(serde_json::json!({
                        "kind": "list_op_count",
                        "frame": f,
                        "list_ord": k,
                        "draws_a": a.0,
                        "dispatches_a": a.1,
                        "draws_b": b.0,
                        "dispatches_b": b.1,
                    }));
                }
            }
        }
    }

    if write_report {
        let report_dir = dir.join("diverge");
        std::fs::create_dir_all(&report_dir).map_err(|e| e.to_string())?;
        let path = report_dir.join("report.json");
        let body = serde_json::json!({
            "dir": dir.display().to_string(),
            "against": against.map(|p| p.display().to_string()),
            "findings": findings,
            "note": "Offline XCLFRAME diverge — not CGBJ GPU multi-run diverge",
        });
        std::fs::write(&path, serde_json::to_string_pretty(&body).unwrap_or_default())
            .map_err(|e| e.to_string())?;
    }

    if json {
        Ok(serde_json::json!({
            "dir": dir.display().to_string(),
            "against": against.map(|p| p.display().to_string()),
            "findings": findings,
            "first_content_split": first_content_split,
            "gpu_multi_run_diverge": "impossible",
            "present_bin": dir.join("present.bin").is_file()
                || dir.join("frames").join("frame_0001.present.bin").is_file(),
            "contents_bin": dir.join("contents.bin").is_file(),
        })
        .to_string())
    } else {
        let mut out = format!(
            "xcl replay diverge {}\n{} finding(s)\n",
            dir.display(),
            findings.len()
        );
        for f in findings.iter().take(32) {
            out.push_str(&format!("  {}\n", f));
        }
        if findings.len() > 32 {
            out.push_str(&format!("  … {} more\n", findings.len() - 32));
        }
        if write_report {
            out.push_str(&format!("wrote {}\\diverge\\report.json\n", dir.display()));
        }
        if dir.join("present.bin").is_file() {
            out.push_str("present.bin: present (session sidecar)\n");
        }
        if dir.join("contents.bin").is_file() {
            out.push_str("contents.bin: present (session sidecar)\n");
        }
        out.push_str(
            "note: offline content/list diverge; GPU multi-run picture diverge IMPOSSIBLE cross-process\n",
        );
        Ok(out)
    }
}

/// Parse `A..B` or a single integer into an inclusive range.
pub fn parse_range(s: &str) -> Result<(u32, u32), String> {
    if let Some((a, b)) = s.split_once("..") {
        let lo: u32 = a.trim().parse().map_err(|_| format!("bad range {s}"))?;
        let hi: u32 = b.trim().parse().map_err(|_| format!("bad range {s}"))?;
        if lo > hi {
            return Err(format!("range {s}: lo > hi"));
        }
        Ok((lo, hi))
    } else {
        let n: u32 = s.parse().map_err(|_| format!("bad range {s}"))?;
        Ok((n, n))
    }
}

/// Parse `x,y` picture points (repeatable).
pub fn parse_point(s: &str) -> Result<(u32, u32), String> {
    let (x, y) = s
        .split_once(',')
        .ok_or_else(|| format!("pixel needs x,y (got {s})"))?;
    Ok((
        x.trim().parse().map_err(|_| format!("bad x in {s}"))?,
        y.trim().parse().map_err(|_| format!("bad y in {s}"))?,
    ))
}

/// Parse `WxH` picture size.
pub fn parse_wh(s: &str) -> Result<(u32, u32), String> {
    let lower = s.to_ascii_lowercase();
    let (w, h) = lower
        .split_once('x')
        .ok_or_else(|| format!("size needs WxH (got {s})"))?;
    Ok((
        w.trim().parse().map_err(|_| format!("bad W in {s}"))?,
        h.trim().parse().map_err(|_| format!("bad H in {s}"))?,
    ))
}

#[allow(dead_code)]
pub fn default_out_hint(dir: &Path) -> PathBuf {
    dir.join("edited")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ops() -> Vec<u8> {
        encode_ops(&[
            ListOp::ListKind(0),
            ListOp::BindIds { pso: 0xA, rs: 0xB },
            ListOp::Heaps(vec![0x10, 0x20]),
            ListOp::TouchVa { va: 0x1000, len: 0 },
            ListOp::Written {
                va: 0x2000,
                end: 0x2100,
            },
            ListOp::Draw {
                indexed: true,
                count: 3,
                instances: 1,
                first: 0,
                base: 0,
                first_instance: 0,
            },
            ListOp::Draw {
                indexed: false,
                count: 6,
                instances: 2,
                first: 0,
                base: 0,
                first_instance: 4,
            },
            ListOp::Dispatch { x: 8, y: 1, z: 1 },
            ListOp::DispatchMesh { x: 2, y: 2, z: 1 },
            ListOp::Indirect {
                sig: 0x50,
                max: 8,
                args: 0x100,
                args_off: 0,
                count: 0x200,
                count_off: 0,
            },
        ])
    }

    #[test]
    fn ops_roundtrip() {
        let raw = sample_ops();
        let decoded = decode_ops(&raw).unwrap();
        assert_eq!(decoded.len(), 10);
        assert!(matches!(
            &decoded[6],
            ListOp::Draw {
                first_instance: 4,
                ..
            }
        ));
        let again = encode_ops(&decoded);
        assert_eq!(decode_ops(&again).unwrap(), decoded);
    }

    #[test]
    fn edit_skips_draws_and_dispatches() {
        let decoded = decode_ops(&sample_ops()).unwrap();
        let (edited, rep) = edit_ops(
            &decoded,
            &EditOpts {
                skip_draws: true,
                skip_dispatches: true,
                ..Default::default()
            },
        );
        assert_eq!(rep.draws_dropped, 2);
        assert_eq!(rep.dispatches_dropped, 3);
        assert!(edited.iter().all(|o| {
            !matches!(
                o,
                ListOp::Draw { .. }
                    | ListOp::Dispatch { .. }
                    | ListOp::DispatchMesh { .. }
                    | ListOp::Indirect { .. }
            )
        }));
    }

    #[test]
    fn meta_only_keeps_bind_heaps() {
        let decoded = decode_ops(&sample_ops()).unwrap();
        let (edited, _) = edit_ops(
            &decoded,
            &EditOpts {
                meta_only: true,
                ..Default::default()
            },
        );
        assert!(edited.iter().any(|o| matches!(o, ListOp::BindIds { .. })));
        assert!(edited.iter().any(|o| matches!(o, ListOp::Heaps(_))));
        assert!(!edited.iter().any(|o| matches!(o, ListOp::Draw { .. })));
    }

    #[test]
    fn point_maps_into_present_rect() {
        let pic = (1920, 1080);
        let rect = [0, 0, 1920, 1080];
        assert_eq!(point_in_rect(0, 0, pic, rect), Some((0, 0)));
        assert_eq!(point_in_rect(1919, 1079, pic, rect), Some((1919, 1079)));
    }

    #[test]
    fn parse_helpers() {
        assert_eq!(parse_range("3..7").unwrap(), (3, 7));
        assert_eq!(parse_point("10,20").unwrap(), (10, 20));
        assert_eq!(parse_wh("1280x720").unwrap(), (1280, 720));
    }

    #[test]
    fn edit_and_pixel_on_temp_capture() {
        let dir = std::env::temp_dir().join(format!("xcl-frame-tools-{}", std::process::id()));
        let frames = dir.join("frames");
        std::fs::create_dir_all(&frames).unwrap();
        let events = vec![
            WireEvent::ListPayload {
                list: 0x40,
                kind: 0,
                ops: sample_ops(),
            },
            WireEvent::ContentSample {
                va: 0x1000,
                len: 4,
                hash: fnv1a32(&[1, 2, 3, 4]),
                sample: vec![1, 2, 3, 4],
            },
            WireEvent::Present {
                frame: 1,
                token: 1,
                submits: 1,
                queue: 0x50,
                planes: vec![frame_wire::PresentPlane {
                    resource: 0x99,
                    rect: Some([0, 0, 64, 64]),
                    width: 64,
                    height: 64,
                    format: 0,
                }],
            },
        ];
        frame_wire::write_frame(&frames.join("frame_0001.bin"), &events).unwrap();
        std::fs::write(dir.join("index.jsonl"), "{\"frame\":1}\n").unwrap();

        let listing = cli_listing(&dir, None, false).unwrap();
        assert!(listing.contains("draw"));

        let edited_out = dir.join("edited");
        let edit = cli_edit(
            &dir,
            &EditOpts {
                skip_draws: true,
                ..Default::default()
            },
            Some(&edited_out),
            None,
            false,
        )
        .unwrap();
        assert!(edit.contains("dropped 1 draw"));
        assert!(edited_out.join("frames").join("frame_0001.bin").is_file());

        let pix = cli_pixel(&dir, &[(1, 1)], Some((64, 64)), None, None, false).unwrap();
        assert!(pix.contains("point"));

        let div = cli_diverge(&dir, None, None, true, false).unwrap();
        assert!(div.contains("finding"));
        assert!(dir.join("diverge").join("report.json").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
