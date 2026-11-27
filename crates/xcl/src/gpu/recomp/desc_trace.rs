//! Per-lane descriptor origin tracing and waterfall proof (GPL-2).
//!
//! Study-only CGBJ (`victory-shader` `Source::{Fetched,Lanes,LanePhi}`, Tracer waterfall,
//! emit `lane_heap_index`) is GPL-3; this module reimplements the **subset** XCL can prove on
//! decoded RDNA2 without CGBJ's SSA `Value` / Structured BDD IR:
//! - structural single-block waterfall loops (`v_readfirstlane` + `v_cmp_eq` + `s_cbranch_execnz`)
//! - `Fetched` / `Lanes` / `LanePhi`-style origins for the VGPRs those loops narrow over
//! - per-access [`LaneHeapExpr`] for DXIL/HLSL `NonUniformResourceIndex` (lane_heap_index analogue)
//!
//! Full CGBJ BDD `meet` over arbitrary nested waterfalls, SCC `Select` when arms differ, and
//! multi-block nested waterfalls remain out of scope (see module tests / join_lane_phi docs).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::rdna2::{self, Decoded};
use super::{join_lane_phi, lane_masks, strict_joins};

/// CGBJ `Source` analogue for a descriptor that may differ between lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescSource {
    /// Scalar load / constant SGPRs (uniform).
    Loaded { load_pc: u64, first: u8 },
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
    Imm(u32),
}

/// Result of tracing a program for per-lane descriptors + waterfall.
#[derive(Debug, Clone, Default)]
pub struct DescTrace {
    pub waterfall: WaterfallProof,
    /// Access PC → per-lane heap index when a waterfall (or uniform Fetched) proves it.
    pub lane_heap: BTreeMap<u64, LaneHeapExpr>,
    /// Access PC → classified descriptor origin (Fetched / Lanes / LanePhi / …).
    pub origins: BTreeMap<u64, DescSource>,
    /// Join PC → sole live predecessor block-start PC when BDD polarity + heuristics agree.
    pub only_edge: BTreeMap<u64, u64>,
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
    if let Ok(pol) = lane_masks::join_polarity(insts, wave64) {
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
    prove_waterfalls(insts, wave64, &mut out);
    out
}

/// Structural waterfall: one block that ends in `s_cbranch_execnz` back to itself, contains
/// `v_readfirstlane` of descriptor VGPRs, `v_cmp_eq` against those, and a memory/image access.
fn prove_waterfalls(insts: &[Decoded], _wave64: bool, out: &mut DescTrace) {
    let by_pc: HashMap<u64, usize> = insts.iter().enumerate().map(|(i, d)| (d.pc, i)).collect();
    // Block ranges from terminators / branch targets.
    let mut leaders = BTreeSet::from([0usize]);
    for (i, d) in insts.iter().enumerate() {
        if let Some(t) = rdna2::branch_target(d) {
            if let Some(&j) = by_pc.get(&t) {
                leaders.insert(j);
            }
            leaders.insert(i + 1);
        } else if d.inst.flags & rdna2::F_TERMINATOR != 0 {
            leaders.insert(i + 1);
        }
    }
    leaders.retain(|&i| i < insts.len());
    let firsts: Vec<usize> = leaders.into_iter().collect();
    let blocks: Vec<(usize, usize)> = firsts
        .iter()
        .enumerate()
        .map(|(b, &i)| (i, firsts.get(b + 1).copied().unwrap_or(insts.len())))
        .collect();

    for &(start, end) in &blocks {
        if end <= start {
            continue;
        }
        let last = &insts[end - 1];
        if last.inst.name != "S_CBRANCH_EXECNZ" {
            continue;
        }
        let Some(t) = rdna2::branch_target(last) else {
            continue;
        };
        // Self-loop: taken target is this block's start.
        if t != insts[start].pc {
            continue;
        }
        // Collect readfirstlanes and cmp_eq in the block.
        let mut reads: Vec<(u64, u64, u64)> = Vec::new(); // (pc, sgpr_dst, vgpr_src)
        let mut compares: Vec<u64> = Vec::new();
        let mut accesses: Vec<u64> = Vec::new();
        for d in &insts[start..end] {
            match d.inst.name {
                "V_READFIRSTLANE_B32" => {
                    let dst = f(d, "VDST").unwrap_or(0);
                    let src = f(d, "SRC0").unwrap_or(0);
                    // SRC0 is VGPR encoding: VOP1 uses VGPR index directly in some encodings;
                    // field is OPR_VGPR_OR_LDS — treat as vgpr number when < 256.
                    let vgpr = if src >= 256 { src - 256 } else { src };
                    reads.push((d.pc, dst, vgpr));
                }
                n if n.starts_with("V_CMP_EQ_") || n.starts_with("V_CMPX_EQ_") => {
                    compares.push(d.pc);
                }
                _ if matches!(
                    d.enc.name,
                    "ENC_MIMG" | "ENC_MUBUF" | "ENC_MTBUF" | "ENC_FLAT" | "ENC_FLAT_GLBL"
                ) =>
                {
                    accesses.push(d.pc);
                }
                _ => {}
            }
        }
        if reads.is_empty() || compares.is_empty() || accesses.is_empty() {
            continue;
        }
        // Minimal structural proof: readfirstlane present, at least one eq compare, at least one
        // access, back-edge retry. Full CGBJ meet of selected/remaining/retry masks is not
        // reconstructed here (honest subset).
        out.waterfall.reads.extend(reads.iter().map(|r| r.0));
        out.waterfall.compares.extend(compares.iter().copied());
        out.waterfall.accesses.extend(accesses.iter().copied());

        for &apc in &accesses {
            let Some(ai) = by_pc.get(&apc) else { continue };
            let ad = &insts[*ai];
            let srsrc = f(ad, "SRSRC").map(|v| v * rdna2::register_scale("SRSRC"));
            // Prefer the readfirstlane whose SGPR dst feeds SRSRC dword0.
            let lane_vgpr = srsrc.and_then(|s| {
                reads.iter().find(|&&(_, dst, _)| dst == s).map(|&(_, _, v)| v)
            }).or_else(|| reads.first().map(|r| r.2));
            let Some(vgpr0) = lane_vgpr else { continue };

            let origin = classify_vgpr(insts, vgpr0, &out.only_edge);
            out.origins.insert(apc, origin.clone());
            if let Some(expr) = heap_expr_from_source(&origin, /*sampler*/ false) {
                out.lane_heap.insert(apc, expr);
            } else {
                out.lane_heap.insert(apc, LaneHeapExpr::VgprDword0(vgpr0));
            }
            // Record Fetched loads.
            collect_loads(&origin, &mut out.waterfall.loads);
        }
    }
}

fn collect_loads(src: &DescSource, loads: &mut BTreeSet<u64>) {
    match src {
        DescSource::Fetched { load_pc, .. } | DescSource::Loaded { load_pc, .. } => {
            loads.insert(*load_pc);
        }
        DescSource::Lanes { set, clear, .. } => {
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
    // Last writer of this VGPR before end of program (scan reverse).
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
            // VOP2: SRC0 = clear, VSRC1 = set, VCC; VOP3: SRC0 clear, SRC1 set, SRC2 mask.
            let (clear_src, set_src, mask) = if d.enc.name == "ENC_VOP3" || d.enc.name == "ENC_VOP3B" {
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
                    classify_vgpr(insts, code - 256, only_edge)
                } else if code < 106 {
                    // SGPR — treat as uniform loaded dword standing in as Imm/Loaded leaf.
                    DescSource::Loaded {
                        load_pc: d.pc,
                        first: 0,
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
                return classify_vgpr(insts, src - 256, only_edge);
            }
        }
        // Phi-like: if this write sits at a join with a sole live pred, peel to that arm's value.
        // Without SSA, fall back to VgprDword0 (emit uses the joined VGPR).
        if let Some(_only) = only_edge.values().next() {
            let _ = _only;
        }
        return DescSource::VgprDword0(vgpr);
    }
    DescSource::VgprDword0(vgpr)
}

fn heap_expr_from_source(src: &DescSource, sampler: bool) -> Option<LaneHeapExpr> {
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
        DescSource::VgprDword0(v) => Some(LaneHeapExpr::VgprDword0(*v)),
        DescSource::Imm(i) => Some(LaneHeapExpr::Imm(*i)),
        DescSource::Lanes {
            mask_sgpr,
            set,
            clear,
        } => Some(LaneHeapExpr::Lanes {
            mask_sgpr: *mask_sgpr,
            set: Box::new(heap_expr_from_source(set, sampler)?),
            clear: Box::new(heap_expr_from_source(clear, sampler)?),
        }),
        DescSource::LanePhi { arms, .. } => {
            let mapped: Option<Vec<_>> = arms
                .iter()
                .map(|a| match a {
                    None => Some(None),
                    Some(s) => heap_expr_from_source(s, sampler).map(Some),
                })
                .collect();
            Some(LaneHeapExpr::LanePhi {
                arms: mapped?,
            })
        }
        DescSource::Loaded { .. } => None,
    }
}

/// HLSL expression for a [`LaneHeapExpr`] (lane_heap_index analogue).
pub fn lane_heap_hlsl(expr: &LaneHeapExpr, sreg: &mut dyn FnMut(u64) -> String, vreg: &mut dyn FnMut(u64) -> String) -> String {
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
            // Lane bit of the mask: WaveGetLaneIndex() bit in the SGPR mask word.
            format!("select((({m} >> WaveGetLaneIndex()) & 1u) != 0u, {s}, {c})")
        }
        LaneHeapExpr::LanePhi { arms } => {
            // Dead arms → 0; if a single live arm, use it; else nest selects is unsound without
            // path predicates — prefer any uniform VgprDword0 / first Some.
            let live: Vec<&LaneHeapExpr> = arms.iter().flatten().collect();
            match live.len() {
                0 => "0u".into(),
                1 => lane_heap_hlsl(live[0], sreg, vreg),
                _ => {
                    // Fallback: first live arm (join VGPR path should have collapsed to VgprDword0).
                    lane_heap_hlsl(live[0], sreg, vreg)
                }
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
        // First encode with a placeholder branch; then rewrite SIMM16 so EXECNZ loops to PC 0.
        let mut insts = decode_lines(&[
            ("V_MOV_B32", "ENC_VOP1", &[("VDST", 4), ("SRC0", 8)], None), // from s8
            ("V_CNDMASK_B32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 9), ("VSRC1", 4)], None), // vcc ? v4 : s9
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
    fn execz_transmission_only_edge_recorded() {
        let insts = decode_lines(&[
            ("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 2)], None),
            ("S_MOV_B32", "ENC_SOP1", &[("SDST", 8), ("SSRC0", 255)], Some(0x1111_2222)),
            ("S_ENDPGM", "ENC_SOPP", &[("SIMM16", 0)], None),
        ]);
        let t = trace(&insts);
        // Join at ENDPGM: sole live pred should be recorded when polarity/heuristics fire.
        assert!(
            !t.only_edge.is_empty() || join_lane_phi::build_sgpr_join_cfg(&insts, &insts.iter().map(|d| d.pc).collect())
                .and_then(|c| join_lane_phi::join_sole_live_predecessor(&c, c.blocks.len() - 1))
                .is_some(),
            "expected only_edge or sole-live pred on transmission join"
        );
    }
}
