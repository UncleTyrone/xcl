//! LanePhi-inspired join handling for constant SGPR descriptor tracking (GPL-2).
//!
//! CGBJ `victory-shader` `Source::LanePhi`, Tracer [`only_edge`], and emit `lane_heap_index`
//! lowering are study-only (GPL-3); XCL implements the **constant-SGPR subset** behind
//! [`super::strict_joins`] (`XCL_STRICT_JOINS`, default on): lane-dead predecessor edges at
//! a join do not intersect away constants the sole live arm defines, matching transmission /
//! alpha-prefilter patterns.
//!
//! **Not ported (honest gap vs full CGBJ):** per-lane heap-index `LanePhi` phis in DXIL/HLSL,
//! `Source::Lanes` / `Fetched`, waterfall tracing, and `Tracer::live` phi peeling.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::rdna2::{self, Decoded};
use super::{block_rpo, join_lane_dead_pred, merge_known_at_join, scalar_constant, strict_joins};

pub(crate) type SgprKnown = HashMap<u64, u32>;

/// Block CFG used for SGPR constant propagation at joins (leaders = basic-block entries).
pub(crate) struct SgprJoinCfg<'a> {
    pub blocks: Vec<&'a [Decoded]>,
    pub next: Vec<Vec<usize>>,
    pub pred: Vec<Vec<usize>>,
    pub rpo: Vec<usize>,
}

pub(crate) fn build_sgpr_join_cfg<'a>(
    insts: &'a [Decoded],
    leaders: &BTreeSet<u64>,
) -> Option<SgprJoinCfg<'a>> {
    let mut starts: Vec<usize> = insts
        .iter()
        .enumerate()
        .filter(|(i, d)| *i == 0 || leaders.contains(&d.pc))
        .map(|(i, _)| i)
        .collect();
    starts.push(insts.len());
    let blocks: Vec<&[Decoded]> = starts
        .windows(2)
        .map(|w| &insts[w[0]..w[1]])
        .filter(|b| !b.is_empty())
        .collect();
    let at: HashMap<u64, usize> = blocks.iter().enumerate().map(|(k, b)| (b[0].pc, k)).collect();
    let next: Vec<Vec<usize>> = blocks
        .iter()
        .enumerate()
        .map(|(k, b)| {
            let d = b.last().unwrap();
            let after = (k + 1 < blocks.len()).then_some(k + 1);
            Some(match (rdna2::branch_target(d), d.inst.name) {
                (Some(t), n) if n.starts_with("S_CBRANCH") => after.into_iter().chain([*at.get(&t)?]).collect(),
                (Some(t), "S_BRANCH" | "S_SETPC_B64") => vec![*at.get(&t)?],
                (Some(_), _) => return None,
                (None, "S_ENDPGM") => Vec::new(),
                (None, _) if d.inst.flags & rdna2::F_TERMINATOR != 0 => return None,
                _ => after.into_iter().collect(),
            })
        })
        .collect::<Option<_>>()?;
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for (k, succs) in next.iter().enumerate() {
        for &n in succs {
            pred[n].push(k);
        }
    }
    let rpo = block_rpo(&next);
    Some(SgprJoinCfg { blocks, next, pred, rpo })
}

/// CGBJ `only_edge` analogue for constant propagation: the unique predecessor that is not
/// lane-dead at this join (loop headers excluded).
pub(crate) fn join_sole_live_predecessor(cfg: &SgprJoinCfg, join_idx: usize) -> Option<usize> {
    if !strict_joins() {
        return None;
    }
    let preds = &cfg.pred[join_idx];
    if preds.len() < 2 {
        return None;
    }
    if preds.iter().any(|&p| cfg.rpo[p] >= cfg.rpo[join_idx]) {
        return None;
    }
    let join_pc = cfg.blocks[join_idx].first().map(|d| d.pc).unwrap_or(0);
    let mut live = Vec::new();
    for &p in preds {
        if !join_lane_dead_pred(cfg.blocks[p], join_pc, join_idx, preds, &cfg.rpo) {
            live.push(p);
        }
    }
    (live.len() == 1).then_some(live[0])
}

/// Merge predecessor `known` into join block `have`, using LanePhi sole-live-arm replacement
/// when exactly one predecessor is live.
pub(crate) fn merge_sgpr_known_at_join(
    cfg: &SgprJoinCfg,
    join_idx: usize,
    pred_idx: usize,
    have: &mut SgprKnown,
    incoming: &SgprKnown,
    dead_edge: bool,
) -> bool {
    if !dead_edge {
        if let Some(only) = join_sole_live_predecessor(cfg, join_idx) {
            if pred_idx == only {
                let changed = have != incoming;
                *have = incoming.clone();
                return changed;
            }
        }
    }
    let before = have.len();
    merge_known_at_join(have, incoming, dead_edge);
    have.len() != before
}

/// Forward-propagate SGPR constants and record four-word descriptors at `record` sites.
pub(crate) fn propagate_sgpr_descriptors<FRun, FRecord>(
    insts: &[Decoded],
    leaders: &BTreeSet<u64>,
    follow: bool,
    mut run_block: FRun,
    mut record: FRecord,
) -> BTreeMap<u64, [u32; 4]>
where
    FRun: FnMut(&[Decoded], &mut SgprKnown, bool),
    FRecord: FnMut(&Decoded, &SgprKnown, &mut BTreeMap<u64, [u32; 4]>),
{
    let mut out = BTreeMap::new();
    let Some(cfg) = build_sgpr_join_cfg(insts, leaders) else {
        let blocks: Vec<&[Decoded]> = {
            let mut starts: Vec<usize> = insts
                .iter()
                .enumerate()
                .filter(|(i, d)| *i == 0 || leaders.contains(&d.pc))
                .map(|(i, _)| i)
                .collect();
            starts.push(insts.len());
            starts
                .windows(2)
                .map(|w| &insts[w[0]..w[1]])
                .filter(|b| !b.is_empty())
                .collect()
        };
        for b in &blocks {
            let mut known = SgprKnown::new();
            run_block(b, &mut known, true);
            for d in *b {
                record(d, &known, &mut out);
            }
        }
        return out;
    };
    if !follow {
        for b in &cfg.blocks {
            let mut known = SgprKnown::new();
            run_block(b, &mut known, true);
            for d in *b {
                record(d, &known, &mut out);
            }
        }
        return out;
    }
    let mut start: Vec<Option<SgprKnown>> = vec![None; cfg.blocks.len()];
    let mut work: Vec<usize> = Vec::new();
    if !cfg.blocks.is_empty() {
        start[0] = Some(SgprKnown::new());
        work.push(0);
    }
    while let Some(k) = work.pop() {
        let mut known = start[k].clone().unwrap_or_default();
        run_block(&cfg.blocks[k], &mut known, false);
        for &n in &cfg.next[k] {
            let join_pc = cfg.blocks[n].first().map(|d| d.pc).unwrap_or(0);
            let dead = join_lane_dead_pred(&cfg.blocks[k], join_pc, n, &cfg.pred[n], &cfg.rpo);
            let changed = match &mut start[n] {
                slot @ None => {
                    *slot = Some(known.clone());
                    true
                }
                Some(have) => merge_sgpr_known_at_join(&cfg, n, k, have, &known, dead),
            };
            if changed {
                work.push(n);
            }
        }
    }
    for (k, b) in cfg.blocks.iter().enumerate() {
        let mut known = start[k].clone().unwrap_or_default();
        run_block(b, &mut known, true);
        for d in *b {
            record(d, &known, &mut out);
        }
    }
    out
}

/// SGPR `s_mov*` / `s_brev` / `s_bfm` tracking inside one block (shared by sampler and SRSRC passes).
pub(crate) fn sgpr_const_step(d: &Decoded, known: &mut SgprKnown) {
    let f = |n: &str| rdna2::field_value(d.enc, &d.words, n);
    let set: Vec<(u64, u32)> = match d.inst.name {
        "S_MOV_B32" => f("SDST")
            .zip(f("SSRC0").and_then(|c| scalar_constant(d, c)))
            .map(|(r, v)| vec![(r, v)])
            .unwrap_or_default(),
        "S_MOVK_I32" => f("SDST")
            .zip(f("SIMM16"))
            .map(|(r, v)| vec![(r, v as u16 as i16 as i32 as u32)])
            .unwrap_or_default(),
        "S_BREV_B32" => f("SDST")
            .zip(f("SSRC0").and_then(|c| scalar_constant(d, c)))
            .map(|(r, v)| vec![(r, v.reverse_bits())])
            .unwrap_or_default(),
        "S_MOV_B64" => match (f("SDST"), f("SSRC0")) {
            (Some(r), Some(c @ 128..=208)) => {
                let v = scalar_constant(d, c).unwrap() as i32 as i64;
                vec![(r, v as u32), (r + 1, (v >> 32) as u32)]
            }
            (Some(r), Some(255)) => d.literal.map(|v| vec![(r, v), (r + 1, 0)]).unwrap_or_default(),
            _ => Vec::new(),
        },
        "S_BFM_B32" => match (
            f("SDST"),
            f("SSRC0").and_then(|c| scalar_constant(d, c)),
            f("SSRC1").and_then(|c| scalar_constant(d, c)),
        ) {
            (Some(r), Some(a), Some(b)) => vec![(r, ((1u32 << (a & 31)) - 1) << (b & 31))],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    for w in rdna2::sgpr_uses(d).1 {
        known.remove(&w);
    }
    for (r, v) in set {
        if r < 106 {
            known.insert(r, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::rdna2;

    fn decode_lines(lines: &[(&str, &str, &[(&str, u64)], Option<u32>)]) -> Vec<Decoded> {
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

    /// Transmission-style join: execz arm is lane-dead; only the fall-through arm defines S#.
    #[test]
    fn sole_live_predecessor_on_execz_transmission_join() {
        let insts = decode_lines(&[
            ("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 1)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 255)], Some(0x1234_5678)),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let leaders: BTreeSet<u64> = insts.iter().map(|d| d.pc).collect();
        let cfg = build_sgpr_join_cfg(&insts, &leaders).expect("transmission cfg");
        let join = cfg.blocks.len() - 1;
        let only = join_sole_live_predecessor(&cfg, join);
        let join_pc = cfg.blocks[join][0].pc;
        let dead = cfg
            .pred[join]
            .iter()
            .copied()
            .filter(|&p| join_lane_dead_pred(cfg.blocks[p], join_pc, join, &cfg.pred[join], &cfg.rpo))
            .collect::<Vec<_>>();
        let live: Vec<_> = cfg.pred[join].iter().copied().filter(|p| !dead.contains(p)).collect();
        assert_eq!(live.len(), 1, "one live arm at transmission join");
        assert_eq!(only, Some(live[0]));
    }

    #[test]
    fn lane_phi_merge_sole_live_replaces_join_state() {
        let insts = decode_lines(&[
            ("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 1)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 255)], Some(0x1234_5678)),
            (
                "IMAGE_SAMPLE_LZ",
                "ENC_MIMG",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 4), ("SSAMP", 2), ("DMASK", 0xF), ("DIM", 1)],
                None,
            ),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let leaders: BTreeSet<u64> = insts.iter().map(|d| d.pc).collect();
        let imm = 0x1234_5678u32;
        let map = constant_samplers_for_test(&insts, &leaders);
        let sample_pc = insts.iter().find(|d| d.enc.name == "ENC_MIMG").unwrap().pc;
        assert_eq!(map.get(&sample_pc).map(|w| w[0]), Some(imm));
    }

    fn constant_samplers_for_test(insts: &[Decoded], leaders: &BTreeSet<u64>) -> BTreeMap<u64, [u32; 4]> {
        super::constant_samplers(insts, leaders, true)
    }
}