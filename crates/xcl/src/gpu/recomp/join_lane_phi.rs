//! LanePhi-inspired join handling for constant SGPR descriptor tracking (GPL-2).
//!
//! CGBJ `victory-shader` `Source::LanePhi`, Tracer [`only_edge`], and emit `lane_heap_index`
//! lowering are study-only (GPL-3); XCL implements the constant-SGPR subset behind
//! [`super::strict_joins`] (`XCL_STRICT_JOINS`, default on) plus per-lane descriptor tracing in
//! [`super::desc_trace`] (Fetched / Lanes / LanePhi origins, waterfall structural proof,
//! lane_heap_index HLSL/DXIL expressions).
//!
//! Depth:
//! - [`only_edge`]-style sole-live predecessor (loop headers refused; BDD polarity optional)
//! - Dead edges do **not** seed join state (CGBJ `None` on unused edges)
//! - [`live_peel_known`]: nested sole-live peel at record time (Tracer `live` / `MAX_JOINS`)
//! - SGPR copies / same-arm `s_cselect` / const SALU followed through blocks
//! - Exec/lane polarity + [`lane_masks`] BDD join-edge liveness + Structured `guarded` PCs
//! - Differing-arm SCC `Select` / Fetched / Lanes / LanePhi live in [`super::desc_trace`]
//! - Waterfall selected/remaining/retry structural meet + VCC/saveexec wiring + per-lane T#
//!
//! **IMPOSSIBLE digression (decoded-RDNA2):** full Structured BDD `Bools::meet` on SSA atoms
//! and IR-level SSA phi reconstruction. Behavioral MATCH: sole-live peel, dead edges → `None`,
//! SGPR follow, structural waterfall + joined-VGPR emit.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::rdna2::{self, Decoded};
use super::{
    block_rpo, join_lane_dead_pred_polarity, lane_masks, merge_known_at_join, scalar_constant,
    strict_joins,
};

pub(crate) type SgprKnown = HashMap<u64, u32>;

/// CGBJ `MAX_JOINS`: nested `only_edge` / `live` peel budget.
pub(crate) const MAX_JOINS: usize = 8;

/// Cap forward worklist steps at joins (pathological CFG fan-in can churn without converging).
const MAX_PROP_STEPS: usize = 1 << 20;

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
/// lane-dead at this join (loop headers excluded). Optional BDD polarity strengthens dead edges.
pub(crate) fn join_sole_live_predecessor(cfg: &SgprJoinCfg, join_idx: usize) -> Option<usize> {
    join_sole_live_predecessor_polarity(cfg, join_idx, None)
}

pub(crate) fn join_sole_live_predecessor_polarity(
    cfg: &SgprJoinCfg,
    join_idx: usize,
    polarity: Option<&BTreeMap<u64, Vec<(u64, bool)>>>,
) -> Option<usize> {
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
        if !join_lane_dead_pred_polarity(
            cfg.blocks[p],
            join_pc,
            join_idx,
            preds,
            &cfg.rpo,
            polarity,
        ) {
            live.push(p);
        }
    }
    (live.len() == 1).then(|| live[0])
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
    merge_sgpr_known_at_join_polarity(cfg, join_idx, pred_idx, have, incoming, dead_edge, None)
}

pub(crate) fn merge_sgpr_known_at_join_polarity(
    cfg: &SgprJoinCfg,
    join_idx: usize,
    pred_idx: usize,
    have: &mut SgprKnown,
    incoming: &SgprKnown,
    dead_edge: bool,
    polarity: Option<&BTreeMap<u64, Vec<(u64, bool)>>>,
) -> bool {
    if !dead_edge {
        if let Some(only) = join_sole_live_predecessor_polarity(cfg, join_idx, polarity) {
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

/// End-of-block SGPR map: `start` constants then `run_block` without recording.
fn block_end_known<FRun>(
    cfg: &SgprJoinCfg,
    start: &[Option<SgprKnown>],
    block: usize,
    run_block: &mut FRun,
) -> SgprKnown
where
    FRun: FnMut(&[Decoded], &mut SgprKnown, bool),
{
    let mut known = start.get(block).and_then(|s| s.clone()).unwrap_or_default();
    run_block(&cfg.blocks[block], &mut known, false);
    known
}

/// Tracer `live` analogue: peel nested sole-live join edges (up to [`MAX_JOINS`]) so a
/// guarded access at a join sees the live arm's SGPR constants even when an earlier seed
/// left the join map thin.
pub(crate) fn live_peel_known<FRun>(
    cfg: &SgprJoinCfg,
    start: &[Option<SgprKnown>],
    block: usize,
    run_block: &mut FRun,
) -> SgprKnown
where
    FRun: FnMut(&[Decoded], &mut SgprKnown, bool),
{
    live_peel_known_polarity(cfg, start, block, run_block, None)
}

pub(crate) fn live_peel_known_polarity<FRun>(
    cfg: &SgprJoinCfg,
    start: &[Option<SgprKnown>],
    mut block: usize,
    run_block: &mut FRun,
    polarity: Option<&BTreeMap<u64, Vec<(u64, bool)>>>,
) -> SgprKnown
where
    FRun: FnMut(&[Decoded], &mut SgprKnown, bool),
{
    let mut known = start.get(block).and_then(|s| s.clone()).unwrap_or_default();
    if !strict_joins() {
        return known;
    }
    let mut peeled = BTreeSet::new();
    for _ in 0..MAX_JOINS {
        let Some(only) = join_sole_live_predecessor_polarity(cfg, block, polarity) else {
            break;
        };
        if !peeled.insert(only) {
            break;
        }
        let arm = block_end_known(cfg, start, only, run_block);
        let mut changed = false;
        for (r, v) in arm {
            match known.get(&r) {
                Some(hv) if *hv == v => {}
                Some(_) => {
                    // Sole-live peel wins over a conflicting join seed (CGBJ `live` arm).
                    known.insert(r, v);
                    changed = true;
                }
                None => {
                    known.insert(r, v);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
        block = only;
    }
    known
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
    let wave64 = insts.iter().any(|d| {
        d.inst.name.contains("SAVEEXEC_B64")
            || (d.inst.name.ends_with("_B64")
                && {
                    let (r, w) = rdna2::sgpr_uses(d);
                    r.iter().chain(w.iter()).any(|&s| s == 126 || s == 127)
                })
    });
    let polarity = lane_masks::join_polarity(insts, wave64).ok();
    let pol = polarity.as_ref();
    let mut start: Vec<Option<SgprKnown>> = vec![None; cfg.blocks.len()];
    let mut work: Vec<usize> = Vec::new();
    if !cfg.blocks.is_empty() {
        start[0] = Some(SgprKnown::new());
        work.push(0);
    }
    let mut steps = 0usize;
    while let Some(k) = work.pop() {
        steps += 1;
        if steps > MAX_PROP_STEPS {
            break;
        }
        let mut known = start[k].clone().unwrap_or_default();
        run_block(&cfg.blocks[k], &mut known, false);
        for &n in &cfg.next[k] {
            let join_pc = cfg.blocks[n].first().map(|d| d.pc).unwrap_or(0);
            let dead = join_lane_dead_pred_polarity(
                &cfg.blocks[k],
                join_pc,
                n,
                &cfg.pred[n],
                &cfg.rpo,
                pol,
            );
            let changed = match &mut start[n] {
                slot @ None => {
                    // CGBJ LanePhi: dead edges contribute `None` — do not seed the join from them.
                    if dead && strict_joins() {
                        false
                    } else {
                        *slot = Some(known.clone());
                        true
                    }
                }
                Some(have) => {
                    merge_sgpr_known_at_join_polarity(&cfg, n, k, have, &known, dead, pol)
                }
            };
            if changed {
                work.push(n);
            }
        }
    }
    for (k, b) in cfg.blocks.iter().enumerate() {
        let mut known = live_peel_known_polarity(&cfg, &start, k, &mut run_block, pol);
        run_block(b, &mut known, true);
        for d in *b {
            record(d, &known, &mut out);
        }
    }
    out
}

/// Resolve a scalar operand to a tracked constant (inline/literal or known SGPR).
fn known_scalar(d: &Decoded, known: &SgprKnown, code: u64) -> Option<u32> {
    if let Some(v) = scalar_constant(d, code) {
        return Some(v);
    }
    (code < 106).then(|| known.get(&code).copied()).flatten()
}

/// SGPR `s_mov*` / copies / same-arm `s_cselect` / const SALU tracking (sampler + SRSRC passes).
pub(crate) fn sgpr_const_step(d: &Decoded, known: &mut SgprKnown) {
    let f = |n: &str| rdna2::field_value(d.enc, &d.words, n);
    let set: Vec<(u64, u32)> = match d.inst.name {
        "S_MOV_B32" => f("SDST")
            .zip(f("SSRC0").and_then(|c| known_scalar(d, known, c)))
            .map(|(r, v)| vec![(r, v)])
            .unwrap_or_default(),
        "S_MOVK_I32" => f("SDST")
            .zip(f("SIMM16"))
            .map(|(r, v)| vec![(r, v as u16 as i16 as i32 as u32)])
            .unwrap_or_default(),
        "S_BREV_B32" => f("SDST")
            .zip(f("SSRC0").and_then(|c| known_scalar(d, known, c)))
            .map(|(r, v)| vec![(r, v.reverse_bits())])
            .unwrap_or_default(),
        "S_NOT_B32" => f("SDST")
            .zip(f("SSRC0").and_then(|c| known_scalar(d, known, c)))
            .map(|(r, v)| vec![(r, !v)])
            .unwrap_or_default(),
        "S_MOV_B64" => match (f("SDST"), f("SSRC0")) {
            (Some(r), Some(c @ 128..=208)) => {
                let v = scalar_constant(d, c).unwrap() as i32 as i64;
                vec![(r, v as u32), (r + 1, (v >> 32) as u32)]
            }
            (Some(r), Some(255)) => d.literal.map(|v| vec![(r, v), (r + 1, 0)]).unwrap_or_default(),
            (Some(r), Some(c)) if c < 106 => match (known.get(&c), known.get(&(c + 1))) {
                (Some(&lo), Some(&hi)) => vec![(r, lo), (r + 1, hi)],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        },
        "S_BFM_B32" => match (
            f("SDST"),
            f("SSRC0").and_then(|c| known_scalar(d, known, c)),
            f("SSRC1").and_then(|c| known_scalar(d, known, c)),
        ) {
            (Some(r), Some(a), Some(b)) => vec![(r, ((1u32 << (a & 31)) - 1) << (b & 31))],
            _ => Vec::new(),
        },
        "S_AND_B32" | "S_OR_B32" | "S_XOR_B32" | "S_NAND_B32" | "S_NOR_B32" | "S_XNOR_B32"
        | "S_ANDN2_B32" | "S_ORN2_B32" => {
            match (
                f("SDST"),
                f("SSRC0").and_then(|c| known_scalar(d, known, c)),
                f("SSRC1").and_then(|c| known_scalar(d, known, c)),
            ) {
                (Some(r), Some(a), Some(b)) => {
                    let v = match d.inst.name {
                        "S_AND_B32" => a & b,
                        "S_OR_B32" => a | b,
                        "S_XOR_B32" => a ^ b,
                        "S_NAND_B32" => !(a & b),
                        "S_NOR_B32" => !(a | b),
                        "S_XNOR_B32" => !(a ^ b),
                        "S_ANDN2_B32" => a & !b,
                        "S_ORN2_B32" => a | !b,
                        _ => unreachable!(),
                    };
                    vec![(r, v)]
                }
                _ => Vec::new(),
            }
        }
        "S_LSHL_B32" | "S_LSHR_B32" | "S_ASHR_I32" => {
            match (
                f("SDST"),
                f("SSRC0").and_then(|c| known_scalar(d, known, c)),
                f("SSRC1").and_then(|c| known_scalar(d, known, c)),
            ) {
                (Some(r), Some(a), Some(b)) => {
                    let s = b & 31;
                    let v = match d.inst.name {
                        "S_LSHL_B32" => a << s,
                        "S_LSHR_B32" => a >> s,
                        "S_ASHR_I32" => ((a as i32) >> s) as u32,
                        _ => unreachable!(),
                    };
                    vec![(r, v)]
                }
                _ => Vec::new(),
            }
        }
        "S_BITSET0_B32" | "S_BITSET1_B32" => match (
            f("SDST"),
            f("SSRC0").and_then(|c| known_scalar(d, known, c)),
        ) {
            (Some(r), Some(bit)) => match known.get(&r).copied() {
                Some(cur) => {
                    let m = 1u32 << (bit & 31);
                    let v = if d.inst.name == "S_BITSET1_B32" {
                        cur | m
                    } else {
                        cur & !m
                    };
                    vec![(r, v)]
                }
                None => Vec::new(),
            },
            _ => Vec::new(),
        },
        // Same-arm cselect/cmov: both outcomes fold to one constant (SCC irrelevant).
        "S_CSELECT_B32" => match (
            f("SDST"),
            f("SSRC0").and_then(|c| known_scalar(d, known, c)),
            f("SSRC1").and_then(|c| known_scalar(d, known, c)),
        ) {
            (Some(r), Some(a), Some(b)) if a == b => vec![(r, a)],
            _ => Vec::new(),
        },
        "S_CMOV_B32" => {
            // Fold only when SCC-set and SCC-clear outcomes agree (dst already holds clear arm).
            match (
                f("SDST"),
                f("SSRC0").and_then(|c| known_scalar(d, known, c)),
            ) {
                (Some(r), Some(a)) => match known.get(&r).copied() {
                    Some(b) if a == b => vec![(r, a)],
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            }
        },
        "S_CSELECT_B64" => match (f("SDST"), f("SSRC0"), f("SSRC1")) {
            (Some(r), Some(a), Some(b)) => {
                match (
                    known_scalar(d, known, a),
                    known_scalar(d, known, a + 1),
                    known_scalar(d, known, b),
                    known_scalar(d, known, b + 1),
                ) {
                    (Some(a0), Some(a1), Some(b0), Some(b1)) if a0 == b0 && a1 == b1 => {
                        vec![(r, a0), (r + 1, a1)]
                    }
                    _ => Vec::new(),
                }
            }
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
    /// `SIMM16` counts dwords after the branch word — skip the literal `s_mov` (2 dwords) to ENDPGM.
    #[test]
    fn sole_live_predecessor_on_execz_transmission_join() {
        let insts = decode_lines(&[
            ("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 2)], None),
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
            .filter(|&p| {
                super::super::join_lane_dead_pred(cfg.blocks[p], join_pc, join, &cfg.pred[join], &cfg.rpo)
            })
            .collect::<Vec<_>>();
        let live: Vec<_> = cfg.pred[join].iter().copied().filter(|p| !dead.contains(p)).collect();
        assert_eq!(live.len(), 1, "one live arm at transmission join");
        assert_eq!(only, Some(live[0]));
    }

    #[test]
    fn lane_phi_merge_sole_live_replaces_join_state() {
        // Match `execz_dead_edge_at_join_keeps_live_arm_sampler`: skip four SGPRs (lit + 3) to the sample.
        let imm = 0x1234_5678u32;
        let insts = decode_lines(&[
            ("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 5)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 255)], Some(imm)),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 9), ("SSRC0", 128)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 10), ("SSRC0", 128)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 11), ("SSRC0", 128)], None),
            (
                "IMAGE_SAMPLE_LZ",
                "ENC_MIMG",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 4), ("SSAMP", 2), ("DMASK", 0xF), ("DIM", 1)],
                None,
            ),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let leaders: BTreeSet<u64> = insts.iter().map(|d| d.pc).collect();
        let map = constant_samplers_for_test(&insts, &leaders);
        let sample_pc = insts.iter().find(|d| d.enc.name == "ENC_MIMG").unwrap().pc;
        assert_eq!(map.get(&sample_pc).map(|w| w[0]), Some(imm));
    }

    /// Copies of known SGPRs must stay constant through a live arm (Tracer origin follow).
    #[test]
    fn sgpr_copy_follow_keeps_sampler_after_execz_join() {
        let imm = 0xA5A5_5A5Au32;
        let insts = decode_lines(&[
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 4), ("SSRC0", 255)], Some(imm)),
            // Skip four dword-sized movs (s8 copy + s9..s11) onto the sample join.
            ("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 4)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 4)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 9), ("SSRC0", 128)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 10), ("SSRC0", 128)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 11), ("SSRC0", 128)], None),
            (
                "IMAGE_SAMPLE_LZ",
                "ENC_MIMG",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 4), ("SSAMP", 2), ("DMASK", 0xF), ("DIM", 1)],
                None,
            ),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let leaders: BTreeSet<u64> = insts.iter().map(|d| d.pc).collect();
        let map = constant_samplers_for_test(&insts, &leaders);
        let sample_pc = insts.iter().find(|d| d.enc.name == "ENC_MIMG").unwrap().pc;
        assert_eq!(map.get(&sample_pc).map(|w| w[0]), Some(imm), "copy of known SGPR must fold");
    }

    /// Same-arm `s_cselect` (both sides equal) keeps the constant through the join.
    #[test]
    fn same_arm_cselect_folds_constant_sampler_word() {
        let imm = 0x1111_2222u32;
        let insts = decode_lines(&[
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 4), ("SSRC0", 255)], Some(imm)),
            ("S_CSELECT_B32", "ENC_SOP2", &[("SDST", 8), ("SSRC0", 4), ("SSRC1", 4)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 9), ("SSRC0", 128)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 10), ("SSRC0", 128)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 11), ("SSRC0", 128)], None),
            (
                "IMAGE_SAMPLE_LZ",
                "ENC_MIMG",
                &[("VDATA", 10), ("VADDR", 0), ("SRSRC", 4), ("SSAMP", 2), ("DMASK", 0xF), ("DIM", 1)],
                None,
            ),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let leaders: BTreeSet<u64> = insts.iter().map(|d| d.pc).collect();
        let map = constant_samplers_for_test(&insts, &leaders);
        let sample_pc = insts.iter().find(|d| d.enc.name == "ENC_MIMG").unwrap().pc;
        assert_eq!(map.get(&sample_pc).map(|w| w[0]), Some(imm));
    }

    fn constant_samplers_for_test(insts: &[Decoded], leaders: &BTreeSet<u64>) -> BTreeMap<u64, [u32; 4]> {
        super::super::constant_samplers(insts, leaders, true)
    }
}
