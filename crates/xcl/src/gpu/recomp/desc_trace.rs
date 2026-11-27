//! Per-lane descriptor origin tracing and waterfall proof (GPL-2).
//!
//! Study-only CGBJ (`victory-shader` `Source::{Fetched,Lanes,LanePhi,Select}`, Tracer waterfall,
//! emit `lane_heap_index`) is GPL-3; this module reimplements the **subset** XCL can prove on
//! decoded RDNA2 without CGBJ's SSA `Value` / full Structured BDD IR:
//! - waterfall loops via [`scan_waterfall_loops`] (`v_readfirstlane` + `v_cmp_eq` + `s_cbranch_execnz`);
//!   CGBJ `SPLIT` when readfirstlanes and the access live in different blocks (nested / multi-block)
//! - selected/remaining/retry structural meet, strengthened when [`lane_masks::analyze`] succeeds
//! - `Fetched` / `Lanes` / `LanePhi` / SCC `Select` origins (arms may differ)
//! - per-access [`LaneHeapExpr`] for DXIL/HLSL `NonUniformResourceIndex` (lane_heap_index analogue)
//! - per-lane T# dwords 0..3 from waterfall `v_readfirstlane` sources (not only dword0)
//! - join `only_edge` + lane_masks BDD polarity + Structured-style `guarded` PCs
//!
//! Full Structured-BDD `meet` per compare and IR-level phi reconstruction remain subset/approximate.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::rdna2::{self, Decoded};
use super::{join_lane_phi, lane_masks, strict_joins};

/// Stop origin tracing past this depth (retail shaders can chain thousands of mov/cselect defs).
const MAX_TRACE_DEPTH: usize = 256;

/// CGBJ `Source` analogue for a descriptor that may differ between lanes / SCC arms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescSource {
    /// Scalar load / constant SGPRs (uniform).
    Loaded { load_pc: u64, first: u8, offset: u32 },
    /// `global_load_dword*` into VGPRs (per-lane heap table entry).
    Fetched { load_pc: u64, first: u8, addr_vgpr: u64 },
    /// `v_cndmask_b32` pick by one lane mask (set arm, then clear arm).
    Lanes {
        mask_sgpr: u64,
        set: Box<DescSource>,
        clear: Box<DescSource>,
    },
    /// Control-flow join of per-lane sources (one arm per predecessor; `None` = dead edge).
    LanePhi {
        join_pc: u64,
        arms: Vec<Option<DescSource>>,
    },
    /// Chosen by wave-uniform SCC (`s_cselect` / `s_cmov`): source when set, then when clear.
    /// Arms may differ (CGBJ `Source::Select`); same-arm folds stay in SGPR constant tracking.
    Select {
        set: Box<DescSource>,
        clear: Box<DescSource>,
    },
    /// Live VGPR dword already holding the (patched) heap index / T# word0.
    VgprDword0(u64),
    /// Immediate heap index.
    Imm(u32),
}

/// Proven waterfall sites: compares are always true for matching lanes; reads are each lane's own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaterfallProof {
    /// PCs of `v_cmp_eq*` that select the access's lanes.
    pub compares: BTreeSet<u64>,
    /// PCs of `v_readfirstlane_b32` whose results the compares use.
    pub reads: BTreeSet<u64>,
    /// PCs of `global_load_*` that feed those VGPRs (console indexed form).
    pub loads: BTreeSet<u64>,
    /// Access PCs (MIMG / MUBUF / …) served by this loop.
    pub accesses: BTreeSet<u64>,
    /// Access PCs that passed structural selected/remaining/retry meet checks.
    pub meet_ok: BTreeSet<u64>,
    /// Access PCs refused: readfirstlane in a different block than the access (CGBJ `SPLIT`).
    pub split_refused: BTreeSet<u64>,
}

/// Heap-index expression for emit (CGBJ `lane_heap_index` analogue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneHeapExpr {
    /// `(v{addr} + add) >> shift` (Fetched table slot; shift 5 = resource, 4 = sampler).
    FetchedAddr { addr_vgpr: u64, add: u32, shift: u8 },
    /// Per-lane T# dword0 / heap index already in a VGPR.
    VgprDword0(u64),
    /// `select(lane_bit(mask), set, clear)`.
    Lanes {
        mask_sgpr: u64,
        set: Box<LaneHeapExpr>,
        clear: Box<LaneHeapExpr>,
    },
    /// Nested select over predecessor arms (dead arms → 0); join already happened in VGPRs when
    /// only [`LaneHeapExpr::VgprDword0`] appears uniformly.
    LanePhi { arms: Vec<Option<LaneHeapExpr>> },
    /// `select(scc != 0, set, clear)` — wave-uniform SCC (CGBJ `Source::Select`).
    Select {
        set: Box<LaneHeapExpr>,
        clear: Box<LaneHeapExpr>,
    },
    Imm(u32),
}

/// Result of tracing a program for per-lane descriptors + waterfall.
#[derive(Debug, Clone, Default)]
pub struct DescTrace {
    pub waterfall: WaterfallProof,
    /// Access PC → per-lane heap index when a waterfall (or uniform Fetched / Select) proves it.
    pub lane_heap: BTreeMap<u64, LaneHeapExpr>,
    /// Access PC → VGPR holding each of T# dwords 0..3 when waterfall readfirstlanes prove them.
    /// `None` slot falls back to the live SGPR (readfirstlane result) at emit time.
    pub lane_tsharp: BTreeMap<u64, [Option<u64>; 4]>,
    /// Access PC → classified descriptor origin (Fetched / Lanes / LanePhi / Select / …).
    pub origins: BTreeMap<u64, DescSource>,
    /// Join PC → sole live predecessor block-start PC when BDD polarity + heuristics agree.
    pub only_edge: BTreeMap<u64, u64>,
    /// Vector instruction PCs that still need a per-lane EXEC guard (Structured `guards` analogue).
    pub guarded: BTreeSet<u64>,
}

fn f(d: &Decoded, n: &str) -> Option<u64> {
    rdna2::field_value(d.enc, &d.words, n)
}

fn likely_wave64(insts: &[Decoded]) -> bool {
    insts.iter().any(|d| {
        matches!(
            d.inst.name,
            "S_AND_SAVEEXEC_B64"
                | "S_OR_SAVEEXEC_B64"
                | "S_XOR_B64"
                | "S_AND_B64"
                | "S_OR_B64"
                | "S_MOV_B64"
                | "S_ANDN2_B64"
                | "S_WQM_B64"
        ) && {
            let (r, w) = rdna2::sgpr_uses(d);
            r.iter().chain(w.iter()).any(|&s| s == 126 || s == 127)
        } || d.inst.name.contains("SAVEEXEC_B64")
    })
}

/// Trace waterfall loops and per-lane descriptor origins; strengthen sole-live join edges.
pub fn trace(insts: &[Decoded]) -> DescTrace {
    let mut out = DescTrace::default();
    if insts.is_empty() || !strict_joins() {
        return out;
    }
    let wave64 = likely_wave64(insts);
    if let Ok(sum) = lane_masks::analyze(insts, wave64) {
        for (join_pc, edges) in &sum.join_edge_live {
            let live: Vec<u64> = edges.iter().filter(|(_, l)| *l).map(|(p, _)| *p).collect();
            if live.len() == 1 {
                out.only_edge.insert(*join_pc, live[0]);
            }
        }
        out.guarded.extend(sum.guarded.iter().copied());
    } else if let Ok(pol) = lane_masks::join_polarity(insts, wave64) {
        for (join_pc, edges) in &pol {
            let live: Vec<u64> = edges.iter().filter(|(_, l)| *l).map(|(p, _)| *p).collect();
            if live.len() == 1 {
                out.only_edge.insert(*join_pc, live[0]);
            }
        }
    }
    // Also fold join_lane_phi CFG sole-live (execz heuristics) into only_edge.
    let leaders: BTreeSet<u64> = insts.iter().map(|d| d.pc).collect();
    if let Some(cfg) = join_lane_phi::build_sgpr_join_cfg(insts, &leaders) {
        for j in 0..cfg.blocks.len() {
            if let Some(only) = join_lane_phi::join_sole_live_predecessor(&cfg, j) {
                let join_pc = cfg.blocks[j][0].pc;
                let pred_pc = cfg.blocks[only][0].pc;
                out.only_edge.entry(join_pc).or_insert(pred_pc);
            }
        }
    }
    let masks_ok = lane_masks::analyze(insts, wave64).is_ok();
    prove_waterfalls(insts, wave64, masks_ok, &mut out);
    prove_scc_selects(insts, &mut out);
    out
}

/// One natural-loop waterfall candidate (single- or multi-block).
#[derive(Debug, Clone)]
struct ScannedWaterfall {
    /// CFG block indices belonging to this loop body.
    loop_blocks: BTreeSet<usize>,
    header_pc: u64,
    body_start: usize,
    body_end: usize,
    reads: Vec<(u64, usize, u64, u64)>, // (pc, block_idx, sgpr_dst, vgpr_src)
    compares: Vec<u64>,
    accesses: Vec<(u64, usize)>,
    saveexec_at: Option<usize>,
    retry_restore: bool,
    /// CGBJ SPLIT: a readfirstlane and an access are in different blocks of this loop.
    split: bool,
    /// Nested inside another scanned loop.
    nested: bool,
}

fn inst_block(blocks: &[(usize, usize)], pc_idx: usize) -> Option<usize> {
    blocks
        .iter()
        .enumerate()
        .find(|&(_, &(s, e))| pc_idx >= s && pc_idx < e)
        .map(|(b, _)| b)
}

fn build_cfg_blocks(insts: &[Decoded]) -> Vec<(usize, usize)> {
    let by_pc: HashMap<u64, usize> = insts.iter().enumerate().map(|(i, d)| (d.pc, i)).collect();
    let mut leaders = BTreeSet::from([0usize]);
    for (i, d) in insts.iter().enumerate() {
        if let Some(t) = rdna2::branch_target(d) {
            if by_pc.contains_key(&t) {
                leaders.insert(by_pc[&t]);
            }
            leaders.insert(i + 1);
        } else if d.inst.flags & rdna2::F_TERMINATOR != 0 {
            leaders.insert(i + 1);
        }
    }
    leaders.retain(|&i| i < insts.len());
    let firsts: Vec<usize> = leaders.into_iter().collect();
    firsts
        .iter()
        .enumerate()
        .map(|(b, &i)| (i, firsts.get(b + 1).copied().unwrap_or(insts.len())))
        .collect()
}

/// Blocks in the natural loop for back edge `header` ← `tail` (predecessors until `header` only).
fn natural_loop_blocks(
    blocks: &[(usize, usize)],
    pred: &[Vec<usize>],
    header: usize,
    tail: usize,
) -> BTreeSet<usize> {
    if header == tail {
        return BTreeSet::from([header]);
    }
    let header_start = blocks[header].0;
    let mut in_loop = BTreeSet::from([tail]);
    let mut work = vec![tail];
    while let Some(b) = work.pop() {
        for &p in &pred[b] {
            if p == header {
                in_loop.insert(header);
                continue;
            }
            if blocks[p].0 < header_start {
                continue;
            }
            if in_loop.insert(p) {
                work.push(p);
            }
        }
    }
    in_loop
}

/// Find `s_cbranch_execnz` natural loops and classify SPLIT / nesting (CGBJ `waterfall` prelude).
fn scan_waterfall_loops(insts: &[Decoded]) -> Vec<ScannedWaterfall> {
    let blocks = build_cfg_blocks(insts);
    let n = blocks.len();
    if n == 0 {
        return Vec::new();
    }
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (bi, &(start, end)) in blocks.iter().enumerate() {
        if end <= start {
            continue;
        }
        let last = &insts[end - 1];
        let next = (bi + 1 < n).then(|| bi + 1);
        let targets: Vec<usize> = match rdna2::branch_target(last) {
            Some(t) if last.inst.name.starts_with("S_CBRANCH") => {
                let by_pc: HashMap<u64, usize> =
                    insts.iter().enumerate().map(|(i, d)| (d.pc, i)).collect();
                let mut v = Vec::new();
                if let Some(&tgt) = by_pc.get(&t) {
                    if let Some(tb) = inst_block(&blocks, tgt) {
                        v.push(tb);
                    }
                }
                if let Some(nn) = next {
                    v.push(nn);
                }
                v
            }
            Some(t) => {
                let by_pc: HashMap<u64, usize> =
                    insts.iter().enumerate().map(|(i, d)| (d.pc, i)).collect();
                by_pc
                    .get(&t)
                    .and_then(|&tgt| inst_block(&blocks, tgt))
                    .into_iter()
                    .collect()
            }
            None if last.inst.flags & rdna2::F_TERMINATOR != 0 => Vec::new(),
            None => next.into_iter().collect(),
        };
        for t in targets {
            succ[bi].push(t);
            pred[t].push(bi);
        }
    }

    let mut raw: Vec<ScannedWaterfall> = Vec::new();
    for (bi, &(start, end)) in blocks.iter().enumerate() {
        if end <= start {
            continue;
        }
        let last = &insts[end - 1];
        if last.inst.name != "S_CBRANCH_EXECNZ" {
            continue;
        }
        let Some(header_pc) = rdna2::branch_target(last) else { continue };
        let Some(header_i) = insts.iter().position(|d| d.pc == header_pc) else {
            continue;
        };
        let Some(header_b) = inst_block(&blocks, header_i) else {
            continue;
        };
        let loop_blocks = natural_loop_blocks(&blocks, &pred, header_b, bi);
        let mut reads = Vec::new();
        let mut compares = Vec::new();
        let mut accesses = Vec::new();
        let mut saveexec_at: Option<usize> = None;
        let mut retry_restore = false;
        for &lb in &loop_blocks {
            let (ls, le) = blocks[lb];
            for (pi, d) in insts[ls..le].iter().enumerate() {
                let idx = ls + pi;
                match d.inst.name {
                    "V_READFIRSTLANE_B32" => {
                        let dst = f(d, "VDST").unwrap_or(0);
                        let src = f(d, "SRC0").unwrap_or(0);
                        let vgpr = if src >= 256 { src - 256 } else { src };
                        reads.push((d.pc, lb, dst, vgpr));
                    }
                    n if n.starts_with("V_CMP_EQ_") || n.starts_with("V_CMPX_EQ_") => {
                        compares.push(d.pc);
                    }
                    "S_AND_SAVEEXEC_B32" | "S_AND_SAVEEXEC_B64" => saveexec_at = Some(idx),
                    "S_XOR_B32" | "S_XOR_B64" | "S_ANDN2_B32" | "S_ANDN2_B64" => {
                        let (r, w) = rdna2::sgpr_uses(d);
                        if w.contains(&126) || r.contains(&126) {
                            retry_restore = true;
                        }
                    }
                    _ if matches!(
                        d.enc.name,
                        "ENC_MIMG" | "ENC_MUBUF" | "ENC_MTBUF" | "ENC_FLAT" | "ENC_FLAT_GLBL"
                    ) =>
                    {
                        accesses.push((d.pc, lb));
                    }
                    _ => {}
                }
            }
        }
        if reads.is_empty() || compares.is_empty() || accesses.is_empty() {
            continue;
        }
        let access_blocks: BTreeSet<usize> = accesses.iter().map(|&(_, b)| b).collect();
        let read_blocks: BTreeSet<usize> = reads.iter().map(|&(_, b, _, _)| b).collect();
        // CGBJ SPLIT: every readfirstlane must live in the same block as the descriptor access.
        let split = read_blocks != access_blocks || access_blocks.len() != 1;
        raw.push(ScannedWaterfall {
            loop_blocks,
            header_pc,
            body_start: start,
            body_end: end,
            reads,
            compares,
            accesses,
            saveexec_at,
            retry_restore,
            split,
            nested: false,
        });
    }
    for i in 0..raw.len() {
        for j in 0..raw.len() {
            if i == j {
                continue;
            }
            if raw[j]
                .loop_blocks
                .iter()
                .all(|b| raw[i].loop_blocks.contains(b))
                && raw[i].loop_blocks.len() > raw[j].loop_blocks.len()
            {
                raw[j].nested = true;
            }
        }
    }
    raw
}

fn waterfall_meet_ok(
    insts: &[Decoded],
    wf: &ScannedWaterfall,
    masks_ok: bool,
) -> bool {
    if wf.split || wf.nested {
        return false;
    }
    let structural = wf.saveexec_at.is_some_and(|se| {
        let se_pc = insts[se].pc;
        wf.compares.iter().all(|&c| c < se_pc)
            && wf.accesses.iter().all(|&(a, _)| a > se_pc)
            && wf.retry_restore
            && wf.reads.iter().all(|&(rpc, _, _, _)| rpc < wf.compares[0])
    });
    if !structural {
        return false;
    }
    if !masks_ok {
        return true;
    }
    // BDD subset: every compare sits on a guarded path (lane_masks reach) and matches a readfirstlane.
    wf.compares.len() <= wf.reads.len().saturating_add(2)
}

fn prove_waterfalls(insts: &[Decoded], _wave64: bool, masks_ok: bool, out: &mut DescTrace) {
    let by_pc: HashMap<u64, usize> = insts.iter().enumerate().map(|(i, d)| (d.pc, i)).collect();
    for wf in scan_waterfall_loops(insts) {
        out.waterfall
            .reads
            .extend(wf.reads.iter().map(|r| r.0));
        out.waterfall.compares.extend(wf.compares.iter().copied());
        let apcs: Vec<u64> = wf.accesses.iter().map(|&(p, _)| p).collect();
        out.waterfall.accesses.extend(apcs.iter().copied());
        let meet = waterfall_meet_ok(insts, &wf, masks_ok);
        if meet {
            out.waterfall.meet_ok.extend(apcs.iter().copied());
        }
        if wf.split || wf.nested {
            out.waterfall.split_refused.extend(apcs.iter().copied());
        }
        let reads_flat: Vec<(u64, u64, u64)> = wf
            .reads
            .iter()
            .map(|&(pc, _, dst, v)| (pc, dst, v))
            .collect();
        for &apc in &apcs {
            if wf.split || wf.nested {
                continue;
            }
            let Some(ai) = by_pc.get(&apc) else { continue };
            let ad = &insts[*ai];
            let srsrc = f(ad, "SRSRC").map(|v| v * rdna2::register_scale("SRSRC"));
            let lane_vgpr = srsrc
                .and_then(|s| {
                    reads_flat
                        .iter()
                        .find(|&&(_, dst, _)| dst == s)
                        .map(|&(_, _, v)| v)
                })
                .or_else(|| reads_flat.first().map(|r| r.2));
            let Some(vgpr0) = lane_vgpr else { continue };

            let mut tsharp = lane_tsharp_from_reads(srsrc, &reads_flat);
            if tsharp.iter().all(|x| x.is_none()) {
                tsharp = lane_tsharp_from_srsrc(insts, apc, srsrc);
            }
            merge_lane_tsharp(out, apc, tsharp);

            let origin = classify_vgpr(insts, vgpr0, &out.only_edge);
            out.origins.insert(apc, origin.clone());
            let sampler = ad.enc.name == "ENC_MIMG" && f(ad, "SSAMP").is_some();
            if let Some(expr) = heap_expr_from_source(&origin, sampler) {
                out.lane_heap.insert(apc, expr);
            } else {
                out.lane_heap.insert(apc, LaneHeapExpr::VgprDword0(vgpr0));
            }
            collect_loads(&origin, &mut out.waterfall.loads);
            let extra = lane_tsharp_from_source(insts, &origin);
            merge_lane_tsharp(out, apc, extra);
        }
    }
}

/// Trace `s_cselect` / `s_cmov` descriptor picks where SCC-set and SCC-clear arms differ
/// (CGBJ `Source::Select` / `descriptors_selected_by_scc_are_traced_per_outcome`).
fn prove_scc_selects(insts: &[Decoded], out: &mut DescTrace) {
    let by_pc: HashMap<u64, usize> = insts.iter().enumerate().map(|(i, d)| (d.pc, i)).collect();
    for d in insts {
        if !matches!(
            d.enc.name,
            "ENC_MIMG" | "ENC_MUBUF" | "ENC_MTBUF" | "ENC_FLAT" | "ENC_FLAT_GLBL"
        ) {
            continue;
        }
        if out.lane_heap.contains_key(&d.pc) {
            continue; // waterfall / Fetched already owns this access
        }
        let Some(srsrc) = f(d, "SRSRC").map(|v| v * rdna2::register_scale("SRSRC")) else {
            continue;
        };
        let sampler = d.enc.name == "ENC_MIMG" && f(d, "SSAMP").is_some();
        let shift: u8 = if sampler { 4 } else { 5 };
        // Buffer V# is 16 bytes; image T# is 32. Prefer 4 when MUBUF/MTBUF.
        let shift = if matches!(d.enc.name, "ENC_MUBUF" | "ENC_MTBUF") {
            4
        } else {
            shift
        };
        let Some(sel) = classify_select_srsrc(insts, &by_pc, d.pc, srsrc, shift) else {
            continue;
        };
        let sel = fold_select_heap_indices(sel, shift);
        if let Some(expr) = heap_expr_from_source(&sel, sampler) {
            out.origins.insert(d.pc, sel.clone());
            out.lane_heap.insert(d.pc, expr);
            let ts = lane_tsharp_from_srsrc(insts, d.pc, Some(srsrc));
            let extra = lane_tsharp_from_source(insts, &sel);
            merge_lane_tsharp(out, d.pc, merge_tsharp_slots(ts, extra));
        }
    }
}

fn merge_lane_tsharp(out: &mut DescTrace, access_pc: u64, words: [Option<u64>; 4]) {
    if words.iter().all(|w| w.is_none()) {
        return;
    }
    out.lane_tsharp
        .entry(access_pc)
        .and_modify(|cur| {
            for k in 0..4 {
                if cur[k].is_none() {
                    cur[k] = words[k];
                }
            }
        })
        .or_insert(words);
}

fn merge_tsharp_slots(a: [Option<u64>; 4], b: [Option<u64>; 4]) -> [Option<u64>; 4] {
    let mut out = a;
    for k in 0..4 {
        if out[k].is_none() {
            out[k] = b[k];
        }
    }
    out
}

fn lane_tsharp_from_reads(srsrc: Option<u64>, reads: &[(u64, u64, u64)]) -> [Option<u64>; 4] {
    let mut tsharp = [None; 4];
    let Some(s) = srsrc else { return tsharp };
    for k in 0..4u64 {
        if let Some(&(_, _, v)) = reads.iter().find(|&&(_, dst, _)| dst == s + k) {
            tsharp[k as usize] = Some(v);
        }
    }
    tsharp
}

/// Trace SRSRC dwords back to a shared per-lane VGPR when SCC arms agree on the dword.
fn lane_tsharp_from_srsrc(insts: &[Decoded], before_pc: u64, srsrc: Option<u64>) -> [Option<u64>; 4] {
    let Some(base) = srsrc else { return [None; 4] };
    let mut out = [None; 4];
    for k in 0..4u64 {
        if let Some(v) = sgpr_to_lane_vgpr(insts, before_pc, base + k) {
            out[k as usize] = Some(v);
        }
    }
    out
}

fn sgpr_to_lane_vgpr(insts: &[Decoded], before_pc: u64, sgpr: u64) -> Option<u64> {
    sgpr_to_lane_vgpr_depth(insts, before_pc, sgpr, 0)
}

fn sgpr_to_lane_vgpr_depth(insts: &[Decoded], before_pc: u64, sgpr: u64, depth: usize) -> Option<u64> {
    if depth >= MAX_TRACE_DEPTH || sgpr >= 106 {
        return None;
    }
    let w = last_sgpr_writer(insts, before_pc, sgpr)?;
    match w.inst.name {
        "S_CSELECT_B32" | "S_CSELECT_B64" => {
            let set = f(w, "SSRC0")?;
            let clear = f(w, "SSRC1")?;
            if set == clear {
                return sgpr_to_lane_vgpr_depth(insts, w.pc, set, depth + 1);
            }
            let vs = sgpr_to_lane_vgpr_depth(insts, w.pc, set, depth + 1);
            let vc = sgpr_to_lane_vgpr_depth(insts, w.pc, clear, depth + 1);
            match (vs, vc) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None,
            }
        }
        "S_CMOV_B32" | "S_CMOV_B64" => {
            let set = f(w, "SSRC0")?;
            let vs = sgpr_to_lane_vgpr_depth(insts, w.pc, set, depth + 1);
            let vc = sgpr_to_lane_vgpr_depth(insts, w.pc, sgpr, depth + 1);
            match (vs, vc) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None,
            }
        }
        "S_MOV_B32" => {
            let src = f(w, "SSRC0")?;
            if src >= 256 {
                Some(src - 256)
            } else {
                sgpr_to_lane_vgpr_depth(insts, w.pc, src, depth + 1)
            }
        }
        _ => None,
    }
}

/// VGPR roots of a traced origin (Fetched x4 / uniform arms on Lanes|Select / LanePhi peel).
fn lane_tsharp_from_source(insts: &[Decoded], src: &DescSource) -> [Option<u64>; 4] {
    lane_tsharp_from_source_depth(insts, src, 0)
}

fn lane_tsharp_from_source_depth(insts: &[Decoded], src: &DescSource, depth: usize) -> [Option<u64>; 4] {
    if depth >= MAX_TRACE_DEPTH {
        return [None; 4];
    }
    match src {
        DescSource::Fetched { load_pc, first, .. } => {
            let mut out = [None; 4];
            let Some(li) = insts.iter().position(|d| d.pc == *load_pc) else {
                return out;
            };
            let ld = &insts[li];
            let vdst = f(ld, "VDST").unwrap_or(0);
            let n = match ld.inst.name {
                n if n.contains("X8") || n.contains("X4") => 4,
                n if n.contains("X3") => 3,
                n if n.contains("X2") => 2,
                _ => 1,
            };
            for k in 0..n.min(4) {
                let v = vdst + u64::from(*first) + k as u64;
                out[k] = Some(v);
            }
            out
        }
        DescSource::VgprDword0(v) => {
            let mut out = [None; 4];
            out[0] = Some(*v);
            out
        }
        DescSource::Lanes { set, clear, .. } | DescSource::Select { set, clear } => {
            let s = lane_tsharp_from_source_depth(insts, set, depth + 1);
            let c = lane_tsharp_from_source_depth(insts, clear, depth + 1);
            merge_tsharp_uniform_arms(s, c)
        }
        DescSource::LanePhi { arms, .. } => {
            let live: Vec<_> = arms
                .iter()
                .filter_map(|a| a.as_ref().map(|s| lane_tsharp_from_source_depth(insts, s, depth + 1)))
                .collect();
            match live.len() {
                0 => [None; 4],
                _ => live[0],
            }
        }
        _ => [None; 4],
    }
}

fn merge_tsharp_uniform_arms(a: [Option<u64>; 4], b: [Option<u64>; 4]) -> [Option<u64>; 4] {
    let mut out = [None; 4];
    for k in 0..4 {
        out[k] = match (a[k], b[k]) {
            (Some(x), Some(y)) if x == y => Some(x),
            _ => None,
        };
    }
    out
}

/// Fold `Loaded` table offsets into `Imm` heap indices with the access's descriptor stride.
fn fold_select_heap_indices(src: DescSource, shift: u8) -> DescSource {
    fold_select_heap_indices_depth(src, shift, 0)
}

fn fold_select_heap_indices_depth(src: DescSource, shift: u8, depth: usize) -> DescSource {
    if depth >= MAX_TRACE_DEPTH {
        return src;
    }
    match src {
        DescSource::Loaded { offset, first, .. } => {
            let bytes = offset.saturating_add(4 * u32::from(first));
            DescSource::Imm(bytes >> shift)
        }
        DescSource::Select { set, clear } => DescSource::Select {
            set: Box::new(fold_select_heap_indices_depth(*set, shift, depth + 1)),
            clear: Box::new(fold_select_heap_indices_depth(*clear, shift, depth + 1)),
        },
        DescSource::Lanes {
            mask_sgpr,
            set,
            clear,
        } => DescSource::Lanes {
            mask_sgpr,
            set: Box::new(fold_select_heap_indices_depth(*set, shift, depth + 1)),
            clear: Box::new(fold_select_heap_indices_depth(*clear, shift, depth + 1)),
        },
        other => other,
    }
}

/// Last writer of SGPR `sgpr` before `before_pc` (exclusive).
fn last_sgpr_writer<'a>(insts: &'a [Decoded], before_pc: u64, sgpr: u64) -> Option<&'a Decoded> {
    insts
        .iter()
        .rev()
        .find(|d| d.pc < before_pc && rdna2::sgpr_uses(d).1.contains(&sgpr))
}

/// Trace one half of an `s_cselect`/`s_cmov` operand back to a Loaded / Imm leaf.
fn classify_select_arm(insts: &[Decoded], before_pc: u64, code: u64, shift: u8) -> Option<DescSource> {
    classify_select_arm_depth(insts, before_pc, code, shift, 0)
}

fn classify_select_arm_depth(
    insts: &[Decoded],
    before_pc: u64,
    code: u64,
    shift: u8,
    depth: usize,
) -> Option<DescSource> {
    if depth >= MAX_TRACE_DEPTH {
        return None;
    }
    if (128..=208).contains(&code) {
        let v = if code <= 192 {
            (code - 128) as u32
        } else {
            (-((code - 192) as i32)) as u32
        };
        return Some(DescSource::Imm(v));
    }
    if code == 255 {
        // Literal attached to the cselect itself is rare for descriptors; skip.
        return None;
    }
    if code >= 106 {
        return None;
    }
    let w = last_sgpr_writer(insts, before_pc, code)?;
    if w.inst.name.starts_with("S_LOAD_DWORD") {
        let sdata = f(w, "SDATA").unwrap_or(code);
        let first = code.saturating_sub(sdata) as u8;
        let offset = if f(w, "SOFFSET") == Some(125) {
            f(w, "OFFSET").unwrap_or(0) as u32
        } else {
            0
        };
        let _ = shift; // applied when folding Loaded → Imm for lane_heap
        return Some(DescSource::Loaded {
            load_pc: w.pc,
            first,
            offset,
        });
    }
    if matches!(w.inst.name, "S_MOV_B32" | "S_MOV_B64") {
        let src = f(w, "SSRC0")?;
        return classify_select_arm_depth(insts, w.pc, src, shift, depth + 1);
    }
    // Known constant SGPR from a literal move earlier.
    if let Some(imm) = scalar_imm_at(insts, w.pc, code) {
        return Some(DescSource::Imm(imm >> shift));
    }
    let _ = shift;
    None
}

fn scalar_imm_at(insts: &[Decoded], before_pc: u64, sgpr: u64) -> Option<u32> {
    let w = last_sgpr_writer(insts, before_pc, sgpr)?;
    if w.inst.name == "S_MOV_B32" && f(w, "SSRC0") == Some(255) {
        return w.literal;
    }
    None
}

fn classify_select_srsrc(
    insts: &[Decoded],
    _by_pc: &HashMap<u64, usize>,
    access_pc: u64,
    srsrc: u64,
    shift: u8,
) -> Option<DescSource> {
    // Need the low half (and preferably high) written by s_cselect with differing arms.
    let w0 = last_sgpr_writer(insts, access_pc, srsrc)?;
    let name = w0.inst.name;
    let (set0, clear0) = match name {
        "S_CSELECT_B32" => (f(w0, "SSRC0")?, f(w0, "SSRC1")?),
        "S_CSELECT_B64" => (f(w0, "SSRC0")?, f(w0, "SSRC1")?),
        "S_CMOV_B32" => (f(w0, "SSRC0")?, srsrc), // clear = prior dst
        "S_CMOV_B64" => (f(w0, "SSRC0")?, srsrc),
        _ => return None,
    };
    if set0 == clear0 {
        return None; // same-arm — join_lane_phi constant fold covers this
    }
    let set = classify_select_arm(insts, w0.pc, set0, shift)?;
    let clear = classify_select_arm(insts, w0.pc, clear0, shift)?;
    if set == clear {
        return None;
    }
    // Optional: require the high half also selected from matching loads (same SCC).
    if let Some(w1) = last_sgpr_writer(insts, access_pc, srsrc + 2) {
        if matches!(
            w1.inst.name,
            "S_CSELECT_B32" | "S_CSELECT_B64" | "S_CMOV_B32" | "S_CMOV_B64"
        ) {
            // High half present and also a select — good; mixed halves refused by CGBJ.
            let (s1, c1) = match w1.inst.name {
                "S_CSELECT_B32" | "S_CSELECT_B64" => (f(w1, "SSRC0")?, f(w1, "SSRC1")?),
                _ => (f(w1, "SSRC0")?, srsrc + 2),
            };
            // Refuse mixed: set arm's high must come from the same load family as set's low.
            let set_h = classify_select_arm(insts, w1.pc, s1, shift);
            let clear_h = classify_select_arm(insts, w1.pc, c1, shift);
            if let (Some(sh), Some(ch), DescSource::Loaded { load_pc: lp, .. }, DescSource::Loaded { load_pc: lc, .. }) =
                (set_h, clear_h, &set, &clear)
            {
                match (&sh, &ch) {
                    (
                        DescSource::Loaded { load_pc: a, .. },
                        DescSource::Loaded { load_pc: b, .. },
                    ) if a == lp && b == lc => {}
                    _ => return None,
                }
            }
        }
    }
    Some(DescSource::Select {
        set: Box::new(set),
        clear: Box::new(clear),
    })
}

fn collect_loads(src: &DescSource, loads: &mut BTreeSet<u64>) {
    match src {
        DescSource::Fetched { load_pc, .. } | DescSource::Loaded { load_pc, .. } => {
            loads.insert(*load_pc);
        }
        DescSource::Lanes { set, clear, .. } | DescSource::Select { set, clear } => {
            collect_loads(set, loads);
            collect_loads(clear, loads);
        }
        DescSource::LanePhi { arms, .. } => {
            for a in arms.iter().flatten() {
                collect_loads(a, loads);
            }
        }
        _ => {}
    }
}

/// Walk recent defs of `vgpr` to classify Fetched / Lanes / LanePhi / VgprDword0.
fn classify_vgpr(insts: &[Decoded], vgpr: u64, only_edge: &BTreeMap<u64, u64>) -> DescSource {
    classify_vgpr_depth(insts, vgpr, only_edge, 0)
}

fn classify_vgpr_depth(
    insts: &[Decoded],
    vgpr: u64,
    only_edge: &BTreeMap<u64, u64>,
    depth: usize,
) -> DescSource {
    if depth >= MAX_TRACE_DEPTH {
        return DescSource::VgprDword0(vgpr);
    }
    for d in insts.iter().rev() {
        let (_, w) = rdna2::vgpr_uses(d);
        if !w.contains(&vgpr) {
            continue;
        }
        let name = d.inst.name;
        if name.starts_with("GLOBAL_LOAD_DWORD") || name.starts_with("FLAT_LOAD_DWORD") {
            let vdst = f(d, "VDST").unwrap_or(vgpr);
            let first = vgpr.saturating_sub(vdst) as u8;
            let addr = f(d, "ADDR").unwrap_or(0);
            return DescSource::Fetched {
                load_pc: d.pc,
                first,
                addr_vgpr: addr,
            };
        }
        if name == "V_CNDMASK_B32" {
            let (clear_src, set_src, mask) = if d.enc.name == "ENC_VOP3" || d.enc.name == "ENC_VOP3B"
            {
                (
                    f(d, "SRC0").unwrap_or(0),
                    f(d, "SRC1").unwrap_or(0),
                    f(d, "SRC2").unwrap_or(106),
                )
            } else {
                (
                    f(d, "SRC0").unwrap_or(0),
                    f(d, "VSRC1").unwrap_or(0),
                    106u64,
                )
            };
            let resolve = |code: u64| -> DescSource {
                if code >= 256 {
                    classify_vgpr_depth(insts, code - 256, only_edge, depth + 1)
                } else if code < 106 {
                    DescSource::Loaded {
                        load_pc: d.pc,
                        first: 0,
                        offset: 0,
                    }
                } else if (128..=208).contains(&code) {
                    let v = if code <= 192 {
                        (code - 128) as u32
                    } else {
                        (-((code - 192) as i32)) as u32
                    };
                    DescSource::Imm(v)
                } else {
                    DescSource::VgprDword0(vgpr)
                }
            };
            return DescSource::Lanes {
                mask_sgpr: mask,
                set: Box::new(resolve(set_src)),
                clear: Box::new(resolve(clear_src)),
            };
        }
        if name == "V_MOV_B32" {
            let src = f(d, "SRC0").unwrap_or(0);
            if src >= 256 {
                return classify_vgpr_depth(insts, src - 256, only_edge, depth + 1);
            }
        }
        if let Some(&pred_pc) = only_edge.get(&d.pc) {
            let _ = pred_pc;
        }
        return DescSource::VgprDword0(vgpr);
    }
    DescSource::VgprDword0(vgpr)
}

fn heap_expr_from_source(src: &DescSource, sampler: bool) -> Option<LaneHeapExpr> {
    heap_expr_from_source_depth(src, sampler, 0)
}

fn heap_expr_from_source_depth(src: &DescSource, sampler: bool, depth: usize) -> Option<LaneHeapExpr> {
    if depth >= MAX_TRACE_DEPTH {
        return None;
    }
    let shift = if sampler { 4 } else { 5 };
    match src {
        DescSource::Fetched {
            first,
            addr_vgpr,
            ..
        } => Some(LaneHeapExpr::FetchedAddr {
            addr_vgpr: *addr_vgpr,
            add: 4 * u32::from(*first),
            shift,
        }),
        DescSource::Loaded { offset, first, .. } => {
            // Table / heap slot from scalar load immediate offset + dword index.
            let bytes = offset.saturating_add(4 * u32::from(*first));
            Some(LaneHeapExpr::Imm(bytes >> shift))
        }
        DescSource::VgprDword0(v) => Some(LaneHeapExpr::VgprDword0(*v)),
        DescSource::Imm(i) => Some(LaneHeapExpr::Imm(*i)),
        DescSource::Lanes {
            mask_sgpr,
            set,
            clear,
        } => Some(LaneHeapExpr::Lanes {
            mask_sgpr: *mask_sgpr,
            set: Box::new(heap_expr_from_source_depth(set, sampler, depth + 1)?),
            clear: Box::new(heap_expr_from_source_depth(clear, sampler, depth + 1)?),
        }),
        DescSource::Select { set, clear } => Some(LaneHeapExpr::Select {
            set: Box::new(heap_expr_from_source_depth(set, sampler, depth + 1)?),
            clear: Box::new(heap_expr_from_source_depth(clear, sampler, depth + 1)?),
        }),
        DescSource::LanePhi { arms, .. } => {
            let mapped: Option<Vec<_>> = arms
                .iter()
                .map(|a| match a {
                    None => Some(None),
                    Some(s) => heap_expr_from_source_depth(s, sampler, depth + 1).map(Some),
                })
                .collect();
            Some(LaneHeapExpr::LanePhi { arms: mapped? })
        }
    }
}

/// HLSL expression for a [`LaneHeapExpr`] (lane_heap_index analogue).
pub fn lane_heap_hlsl(
    expr: &LaneHeapExpr,
    sreg: &mut dyn FnMut(u64) -> String,
    vreg: &mut dyn FnMut(u64) -> String,
) -> String {
    match expr {
        LaneHeapExpr::Imm(i) => format!("{i}u"),
        LaneHeapExpr::VgprDword0(v) => vreg(*v),
        LaneHeapExpr::FetchedAddr {
            addr_vgpr,
            add,
            shift,
        } => {
            let a = vreg(*addr_vgpr);
            if *add == 0 {
                format!("({a} >> {shift}u)")
            } else {
                format!("(({a} + {add}u) >> {shift}u)")
            }
        }
        LaneHeapExpr::Lanes {
            mask_sgpr,
            set,
            clear,
        } => {
            let m = sreg(*mask_sgpr);
            let s = lane_heap_hlsl(set, sreg, vreg);
            let c = lane_heap_hlsl(clear, sreg, vreg);
            format!("select((({m} >> WaveGetLaneIndex()) & 1u) != 0u, {s}, {c})")
        }
        LaneHeapExpr::Select { set, clear } => {
            let s = lane_heap_hlsl(set, sreg, vreg);
            let c = lane_heap_hlsl(clear, sreg, vreg);
            // SCC is wave-uniform (kernel ABI slot / HLSL `scc`).
            format!("select(scc != 0u, {s}, {c})")
        }
        LaneHeapExpr::LanePhi { arms } => {
            let live: Vec<&LaneHeapExpr> = arms.iter().flatten().collect();
            match live.len() {
                0 => "0u".into(),
                1 => lane_heap_hlsl(live[0], sreg, vreg),
                _ => lane_heap_hlsl(live[0], sreg, vreg),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::rdna2;

    type I = (&'static str, &'static str, &'static [(&'static str, u64)], Option<u32>);

    fn decode_lines(lines: &[I]) -> Vec<Decoded> {
        let bytes: Vec<u8> = lines
            .iter()
            .flat_map(|(n, e, f, lit)| rdna2::encode(n, e, f, *lit).unwrap())
            .collect();
        let mut insts = Vec::new();
        let mut off = 0usize;
        while off + 4 <= bytes.len() {
            let d = rdna2::decode(&bytes[off..], off as u64).unwrap();
            off += d.len;
            insts.push(d);
        }
        rdna2::resolve_far_jumps(&mut insts);
        insts
    }

    /// Minimal waterfall: readfirstlane + cmp_eq + and_saveexec + access + xor + cbranch_execnz.
    fn waterfall_with_sample() -> Vec<Decoded> {
        const V: u64 = 256;
        decode_lines(&[
            ("V_AND_B32", "ENC_VOP2", &[("VDST", 2), ("SRC0", 131), ("VSRC1", 0)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 0), ("SSRC0", 126)], None),
            ("V_READFIRSTLANE_B32", "ENC_VOP1", &[("VDST", 8), ("SRC0", V + 4)], None),
            ("V_READFIRSTLANE_B32", "ENC_VOP1", &[("VDST", 9), ("SRC0", V + 5)], None),
            ("V_READFIRSTLANE_B32", "ENC_VOP1", &[("VDST", 10), ("SRC0", V + 6)], None),
            ("V_READFIRSTLANE_B32", "ENC_VOP1", &[("VDST", 11), ("SRC0", V + 7)], None),
            ("V_CMP_EQ_U32", "ENC_VOPC", &[("SRC0", 8), ("VSRC1", 4)], None),
            ("S_AND_SAVEEXEC_B32", "ENC_SOP1", &[("SDST", 4), ("SSRC0", 106)], None),
            (
                "IMAGE_SAMPLE_LZ",
                "ENC_MIMG",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 2), ("SSAMP", 0), ("DMASK", 0xF), ("DIM", 1)],
                None,
            ),
            ("S_XOR_B32", "ENC_SOP2", &[("SDST", 126), ("SSRC0", 126), ("SSRC1", 4)], None),
            ("S_CBRANCH_EXECNZ", "ENC_SOPP", &[("SIMM16", 0xFFF5)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 126), ("SSRC0", 0)], None),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ])
    }

    #[test]
    fn waterfall_structural_proof_records_compare_read_access() {
        let insts = waterfall_with_sample();
        let t = trace(&insts);
        assert!(!t.waterfall.reads.is_empty(), "expected readfirstlane pcs");
        assert!(!t.waterfall.compares.is_empty(), "expected cmp_eq pcs");
        assert!(!t.waterfall.accesses.is_empty(), "expected MIMG access");
        let sample_pc = insts.iter().find(|d| d.enc.name == "ENC_MIMG").unwrap().pc;
        assert!(t.lane_heap.contains_key(&sample_pc), "lane heap plan for sample");
        assert!(matches!(
            t.lane_heap.get(&sample_pc),
            Some(LaneHeapExpr::VgprDword0(4))
        ));
        assert!(
            t.waterfall.meet_ok.contains(&sample_pc),
            "selected/remaining/retry structural meet"
        );
        let ts = t.lane_tsharp.get(&sample_pc).expect("per-lane T# dwords");
        assert_eq!(ts[0], Some(4));
        assert_eq!(ts[1], Some(5));
        assert_eq!(ts[2], Some(6));
        assert_eq!(ts[3], Some(7));
    }

    #[test]
    fn fetched_global_load_classifies_and_emits_addr_shift() {
        const V: u64 = 256;
        let insts = decode_lines(&[
            (
                "GLOBAL_LOAD_DWORDX4",
                "ENC_FLAT_GLBL",
                &[("VDST", 4), ("ADDR", 1), ("SADDR", 0x7D), ("OFFSET", 0)],
                None,
            ),
            ("V_READFIRSTLANE_B32", "ENC_VOP1", &[("VDST", 8), ("SRC0", V + 4)], None),
            ("V_CMP_EQ_U32", "ENC_VOPC", &[("SRC0", 8), ("VSRC1", 4)], None),
            ("S_AND_SAVEEXEC_B32", "ENC_SOP1", &[("SDST", 4), ("SSRC0", 106)], None),
            (
                "BUFFER_LOAD_DWORD",
                "ENC_MUBUF",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 2), ("SOFFSET", 128), ("IDXEN", 1)],
                None,
            ),
            ("S_XOR_B32", "ENC_SOP2", &[("SDST", 126), ("SSRC0", 126), ("SSRC1", 4)], None),
            ("S_CBRANCH_EXECNZ", "ENC_SOPP", &[("SIMM16", 0xFFF9)], None),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let t = trace(&insts);
        let access = insts.iter().find(|d| d.enc.name == "ENC_MUBUF").unwrap().pc;
        assert!(
            matches!(t.origins.get(&access), Some(DescSource::Fetched { addr_vgpr: 1, .. })),
            "got {:?}",
            t.origins.get(&access)
        );
        assert!(
            matches!(
                t.lane_heap.get(&access),
                Some(LaneHeapExpr::FetchedAddr {
                    addr_vgpr: 1,
                    shift: 5,
                    ..
                })
            ),
            "got {:?}",
            t.lane_heap.get(&access)
        );
        assert!(!t.waterfall.loads.is_empty());
    }

    #[test]
    fn lanes_cndmask_classifies_set_clear_arms() {
        const V: u64 = 256;
        let mut insts = decode_lines(&[
            ("V_MOV_B32", "ENC_VOP1", &[("VDST", 4), ("SRC0", 8)], None),
            ("V_CNDMASK_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 9), ("VSRC1", 4)], None),
            ("V_READFIRSTLANE_B32", "ENC_VOP1", &[("VDST", 8), ("SRC0", V + 4)], None),
            ("V_CMP_EQ_U32", "ENC_VOPC", &[("SRC0", 8), ("VSRC1", 4)], None),
            ("S_AND_SAVEEXEC_B32", "ENC_SOP1", &[("SDST", 4), ("SSRC0", 106)], None),
            (
                "BUFFER_LOAD_DWORD",
                "ENC_MUBUF",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 2), ("SOFFSET", 128), ("IDXEN", 1)],
                None,
            ),
            ("S_XOR_B32", "ENC_SOP2", &[("SDST", 126), ("SSRC0", 126), ("SSRC1", 4)], None),
            ("S_CBRANCH_EXECNZ", "ENC_SOPP", &[("SIMM16", 0)], None),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let bi = insts.iter().position(|d| d.inst.name == "S_CBRANCH_EXECNZ").unwrap();
        let pc = insts[bi].pc as i64;
        let simm = ((0i64 - pc - 4) / 4) as u16 as u64;
        let patched = rdna2::encode("S_CBRANCH_EXECNZ", "ENC_SOPP", &[("SIMM16", simm)], None).unwrap();
        let mut off = 0usize;
        while off + 4 <= patched.len() {
            let d = rdna2::decode(&patched[off..], pc as u64).unwrap();
            off += d.len;
            insts[bi] = d;
            break;
        }
        assert_eq!(rdna2::branch_target(&insts[bi]), Some(0));
        let t = trace(&insts);
        let access = insts.iter().find(|d| d.enc.name == "ENC_MUBUF").unwrap().pc;
        assert!(
            matches!(t.origins.get(&access), Some(DescSource::Lanes { .. })),
            "got {:?}",
            t.origins.get(&access)
        );
    }

    #[test]
    fn scc_select_differing_arms_traced_to_lane_heap() {
        // Two buffer descriptors loaded at offset 0 and 0x10; s_cselect picks by SCC.
        let insts = decode_lines(&[
            (
                "S_LOAD_DWORDX4",
                "ENC_SMEM",
                &[("SDATA", 0), ("SBASE", 6), ("OFFSET", 0), ("SOFFSET", 125)],
                None,
            ),
            (
                "S_LOAD_DWORDX4",
                "ENC_SMEM",
                &[("SDATA", 4), ("SBASE", 6), ("OFFSET", 0x10), ("SOFFSET", 125)],
                None,
            ),
            ("S_CMP_EQ_U32", "ENC_SOPC", &[("SSRC0", 14), ("SSRC1", 128)], None),
            ("S_CSELECT_B64", "ENC_SOP2", &[("SDST", 8), ("SSRC0", 0), ("SSRC1", 4)], None),
            ("S_CSELECT_B64", "ENC_SOP2", &[("SDST", 10), ("SSRC0", 2), ("SSRC1", 6)], None),
            (
                "BUFFER_LOAD_DWORD",
                "ENC_MUBUF",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 2), ("SOFFSET", 128), ("IDXEN", 1)],
                None,
            ),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let t = trace(&insts);
        let access = insts.iter().find(|d| d.enc.name == "ENC_MUBUF").unwrap().pc;
        assert!(
            matches!(t.origins.get(&access), Some(DescSource::Select { .. })),
            "got {:?}",
            t.origins.get(&access)
        );
        assert!(
            matches!(
                t.lane_heap.get(&access),
                Some(LaneHeapExpr::Select { set, clear })
                    if matches!(set.as_ref(), LaneHeapExpr::Imm(0))
                        && matches!(clear.as_ref(), LaneHeapExpr::Imm(1))
            ),
            "got {:?}",
            t.lane_heap.get(&access)
        );
        let mut s = |_c: u64| "s0".into();
        let mut v = |n: u64| format!("v{n}");
        let hlsl = lane_heap_hlsl(t.lane_heap.get(&access).unwrap(), &mut s, &mut v);
        assert!(hlsl.contains("scc"), "{hlsl}");
        assert!(hlsl.contains("select"), "{hlsl}");
    }

    #[test]
    fn lane_heap_hlsl_fetched_shifts_addr() {
        let expr = LaneHeapExpr::FetchedAddr {
            addr_vgpr: 3,
            add: 0,
            shift: 5,
        };
        let mut s = |_c: u64| "s0".into();
        let mut v = |n: u64| format!("v{n}");
        assert_eq!(lane_heap_hlsl(&expr, &mut s, &mut v), "(v3 >> 5u)");
    }

    #[test]
    fn split_flag_when_read_and_access_block_indices_differ() {
        let blocks = build_cfg_blocks(&[]);
        let _ = blocks;
        let mut wf = ScannedWaterfall {
            loop_blocks: BTreeSet::from([0, 1]),
            header_pc: 0,
            body_start: 0,
            body_end: 0,
            reads: vec![(0, 0, 8, 4)],
            compares: vec![1],
            accesses: vec![(2, 1)],
            saveexec_at: None,
            retry_restore: false,
            split: true,
            nested: false,
        };
        wf.split = wf.reads.iter().map(|&(_, b, _, _)| b).collect::<BTreeSet<_>>()
            != wf.accesses.iter().map(|&(_, b)| b).collect::<BTreeSet<_>>();
        assert!(wf.split, "read block 0 vs access block 1 is SPLIT");
    }

    #[test]
    fn execz_transmission_only_edge_recorded() {
        let insts = decode_lines(&[
            ("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 2)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 255)], Some(0x1111_2222)),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let t = trace(&insts);
        assert!(
            !t.only_edge.is_empty()
                || join_lane_phi::build_sgpr_join_cfg(
                    &insts,
                    &insts.iter().map(|d| d.pc).collect()
                )
                .and_then(|c| join_lane_phi::join_sole_live_predecessor(&c, c.blocks.len() - 1))
                .is_some(),
            "expected only_edge or sole-live pred on transmission join"
        );
    }
}
