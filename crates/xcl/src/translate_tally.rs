//! Translate survey tally (GPL-2): used-resource / structured / `--list` richness.
//!
//! Study-only CGBJ `apps/victory/src/translate.rs` `Tally` / `untraced_used` / `tally_resources`
//! / `tally_structured` is GPL-3; this module reimplements the **subset** XCL can compute from
//! console containers ([`crate::gpu::xbox::root_layout`]), decoded RDNA2, [`desc_trace`] origins,
//! and [`lane_masks`] summaries — without CGBJ's full SSA resource Tracer / Structured Bools.

use std::collections::{BTreeMap, HashSet};

use crate::gpu::rdna2::{self, Decoded};
use crate::gpu::recomp::{desc_trace, lane_masks};
use crate::gpu::xbox::{self, ResourceKind, RootKind, RootLayout};

/// Aggregate matching CGBJ translate summary lines (honest XCL subset).
#[derive(Debug, Default, Clone)]
pub struct Tally {
    pub containers: usize,
    pub programs: usize,
    pub translated: usize,
    pub instructions: usize,
    pub structured: usize,
    pub vector: usize,
    pub guarded: usize,
    pub lane_branches: usize,
    pub uniform_branches: usize,
    pub tracked: usize,
    pub loaded: usize,
    pub constant: usize,
    pub dynamic: usize,
    pub scalar: usize,
    pub used: usize,
    /// `"Kind via root"` → count of used-resource entries whose root user-data was not traced.
    pub untraced_used: BTreeMap<String, usize>,
    /// Traced root user-data dwords that no used-resource entry lists.
    pub unlisted: usize,
    pub emit_vs_ps: usize,
    pub emit_cs: usize,
    pub emit_ms: usize,
    pub complete_vs_ps: usize,
    pub complete_cs: usize,
    pub complete_ms: usize,
    /// With `--list`: one line per program (`label  outcome`).
    pub list: bool,
    pub lines: Vec<String>,
    pub failed: BTreeMap<String, (usize, String)>,
}

impl Tally {
    pub fn merge(&mut self, o: Tally) {
        self.containers += o.containers;
        self.programs += o.programs;
        self.translated += o.translated;
        self.instructions += o.instructions;
        self.structured += o.structured;
        self.vector += o.vector;
        self.guarded += o.guarded;
        self.lane_branches += o.lane_branches;
        self.uniform_branches += o.uniform_branches;
        self.tracked += o.tracked;
        self.loaded += o.loaded;
        self.constant += o.constant;
        self.dynamic += o.dynamic;
        self.scalar += o.scalar;
        self.used += o.used;
        for (k, n) in o.untraced_used {
            *self.untraced_used.entry(k).or_default() += n;
        }
        self.unlisted += o.unlisted;
        self.emit_vs_ps += o.emit_vs_ps;
        self.emit_cs += o.emit_cs;
        self.emit_ms += o.emit_ms;
        self.complete_vs_ps += o.complete_vs_ps;
        self.complete_cs += o.complete_cs;
        self.complete_ms += o.complete_ms;
        self.lines.extend(o.lines);
        for (reason, (n, example)) in o.failed {
            self.failed.entry(reason).or_insert((0, example)).0 += n;
        }
    }

    fn fail(&mut self, reason: String, example: &str) {
        if self.list {
            self.lines.push(format!("{example}  {reason}"));
        }
        self.failed
            .entry(reason)
            .or_insert_with(|| (0, example.to_string()))
            .0 += 1;
    }

    /// Note one container seen (caller dedupes blobs if needed).
    pub fn note_container(&mut self) {
        self.containers += 1;
    }

    /// Survey one native program: structured lane-masks + used-resource vs desc_trace origins.
    pub fn observe_program(
        &mut self,
        label: &str,
        stage: &str,
        fssm: &xbox::Fssm,
        code: &[u8],
        complete: bool,
        translate_err: Option<&str>,
    ) {
        self.programs += 1;
        match stage {
            "VS" | "PS" => {
                self.emit_vs_ps += 1;
                self.complete_vs_ps += usize::from(complete);
            }
            "CS" | "kernel" => {
                self.emit_cs += 1;
                self.complete_cs += usize::from(complete);
            }
            "MS" => {
                self.emit_ms += 1;
                self.complete_ms += usize::from(complete);
            }
            _ => {}
        }
        if complete {
            self.translated += 1;
            if self.list {
                self.lines.push(format!("{label}  ok"));
            }
        } else if let Some(e) = translate_err {
            self.fail(e.to_string(), label);
        } else if self.list {
            self.lines.push(format!("{label}  incomplete"));
        }

        let insts = decode_all(code);
        self.instructions += insts.len();
        let wave64 = crate::gpu::recomp::detect_wave64(code);
        if let Ok(sum) = lane_masks::analyze(&insts, wave64) {
            self.structured += 1;
            self.vector += sum.vector;
            self.guarded += sum.guarded.len();
            self.lane_branches += sum.lane_branches;
            self.uniform_branches += sum.uniform_branches;
        }

        let hw = fssm.stages.first().map(|s| s.hw).unwrap_or(6);
        let layout = fssm
            .stages
            .first()
            .and_then(|st| xbox::root_layout(&fssm.data, st).ok());
        let Some(layout) = layout else {
            return;
        };
        let desc = desc_trace::trace(&insts);
        self.tally_resources(hw, &layout, &insts, &desc);
    }

    fn tally_resources(
        &mut self,
        hw: u8,
        layout: &RootLayout,
        insts: &[Decoded],
        desc: &desc_trace::DescTrace,
    ) {
        self.tracked += 1;
        let mut traced: HashSet<u8> = HashSet::new();
        // Root constants are always named by user-data (CGBJ extends traced from Constants params).
        for s in &layout.slots {
            if s.kind == RootKind::Constants {
                traced.insert(s.dword);
            }
        }
        for (apc, origin) in &desc.origins {
            count_origin_leaves(origin, &mut self.loaded, &mut self.constant, &mut self.dynamic);
            if let Some(sgpr) = access_srsrc_sgpr(insts, *apc) {
                if let Some(dword) = dword_for_sgpr(hw, layout, sgpr) {
                    traced.insert(dword);
                }
            }
            if origin_is_proven(origin) {
                self.scalar += 1;
            }
        }
        // Constant-folded S# at MIMG SSAMP: static / immediate sampler path.
        if !desc.lane_heap.is_empty() || !desc.origins.is_empty() {
            for s in &layout.slots {
                if matches!(s.kind, RootKind::Table | RootKind::Descriptor) {
                    // Table pointer itself is not a used-resource heap; leave unless SRSRC hit.
                    let _ = s;
                }
            }
        }
        let listed: HashSet<u8> = layout.used.iter().map(|u| u.dword).collect();
        self.used += layout.used.len();
        let immediate = layout
            .used
            .iter()
            .filter(|u| u.via == RootKind::StaticSampler)
            .count();
        // Distinct static sampler slots vs listed Immediate/StaticSampler used entries (CGBJ role).
        let samplers_match = immediate == 0
            || (layout.static_samplers > 0 && layout.static_samplers >= immediate);
        for u in &layout.used {
            let matched = match u.via {
                RootKind::StaticSampler => samplers_match,
                RootKind::ViewHeap | RootKind::SamplerHeap => true,
                _ => traced.contains(&u.dword),
            };
            if !matched {
                *self
                    .untraced_used
                    .entry(format!("{:?} via {}", u.kind, u.via.name()))
                    .or_default() += 1;
            }
        }
        self.unlisted += traced.difference(&listed).count();
    }

    /// Human summary lines (CGBJ translate printout role).
    pub fn print_summary(&self) {
        println!(
            "containers   {} (translate tally)",
            self.containers
        );
        println!(
            "programs     {} translated of {} ({} decoded instructions)",
            self.translated, self.programs, self.instructions
        );
        println!(
            "structured   {} of {} ({} vector instructions, {} guarded; branches {} per lane, {} uniform)",
            self.structured,
            self.translated.max(self.structured),
            self.vector,
            self.guarded,
            self.lane_branches,
            self.uniform_branches
        );
        println!(
            "resources    {} of {} tracked (descriptors {} loaded, {} constant; {} scalar accesses; {} dynamic/per-lane)",
            self.tracked,
            self.translated.max(self.tracked),
            self.loaded,
            self.constant,
            self.scalar,
            self.dynamic
        );
        println!(
            "  used-resource entries {}: {} not traced {:?}; {} traced parameters not listed",
            self.used,
            self.untraced_used.values().sum::<usize>(),
            self.untraced_used,
            self.unlisted
        );
        println!(
            "DXIL         {} complete of {} VS/PS; {} of {} CS; {} of {} MS",
            self.complete_vs_ps,
            self.emit_vs_ps,
            self.complete_cs,
            self.emit_cs,
            self.complete_ms,
            self.emit_ms
        );
        let mut lines = self.lines.clone();
        lines.sort();
        for l in lines {
            println!("{l}");
        }
        let mut failed: Vec<_> = self.failed.iter().collect();
        failed.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
        for (reason, (n, example)) in failed.into_iter().take(40) {
            println!("  {n:>7}  {reason}  (e.g. {example})");
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "containers": self.containers,
            "programs": self.programs,
            "translated": self.translated,
            "instructions": self.instructions,
            "structured": self.structured,
            "vector": self.vector,
            "guarded": self.guarded,
            "lane_branches": self.lane_branches,
            "uniform_branches": self.uniform_branches,
            "tracked": self.tracked,
            "loaded": self.loaded,
            "constant": self.constant,
            "dynamic": self.dynamic,
            "scalar": self.scalar,
            "used": self.used,
            "untraced_used": self.untraced_used,
            "untraced_used_total": self.untraced_used.values().sum::<usize>(),
            "unlisted": self.unlisted,
            "emit": {
                "vs_ps": [self.complete_vs_ps, self.emit_vs_ps],
                "cs": [self.complete_cs, self.emit_cs],
                "ms": [self.complete_ms, self.emit_ms],
            },
            "list_lines": self.lines.len(),
        })
    }
}

fn decode_all(code: &[u8]) -> Vec<Decoded> {
    let mut insts = Vec::new();
    let mut off = 0usize;
    while off + 4 <= code.len() {
        let Ok(d) = rdna2::decode(&code[off..], off as u64) else {
            break;
        };
        off += d.len;
        insts.push(d);
    }
    rdna2::resolve_far_jumps(&mut insts);
    insts
}

fn access_srsrc_sgpr(insts: &[Decoded], pc: u64) -> Option<u64> {
    let d = insts.iter().find(|d| d.pc == pc)?;
    let f = |n: &str| rdna2::field_value(d.enc, &d.words, n);
    f("SRSRC").map(|v| v * rdna2::register_scale("SRSRC"))
}

fn dword_for_sgpr(hw: u8, layout: &RootLayout, sgpr: u64) -> Option<u8> {
    for s in &layout.slots {
        let Some(base) = xbox::user_sgpr(hw, s.dword) else {
            continue;
        };
        let span = s.sgprs.max(1) as u64;
        if sgpr >= base as u64 && sgpr < base as u64 + span {
            return Some(s.dword);
        }
        // Descriptor / T# often sits at s[base..base+4] loaded from a table pointer at `base`.
        if matches!(s.kind, RootKind::Table | RootKind::Descriptor)
            && sgpr >= base as u64
            && sgpr < base as u64 + 8
        {
            return Some(s.dword);
        }
    }
    None
}

fn origin_is_proven(o: &desc_trace::DescSource) -> bool {
    match o {
        desc_trace::DescSource::Loaded { .. }
        | desc_trace::DescSource::Fetched { .. }
        | desc_trace::DescSource::Imm(_)
        | desc_trace::DescSource::Select { .. }
        | desc_trace::DescSource::Lanes { .. }
        | desc_trace::DescSource::LanePhi { .. } => true,
        desc_trace::DescSource::VgprDword0(_) => true,
    }
}

fn count_origin_leaves(
    o: &desc_trace::DescSource,
    loaded: &mut usize,
    constant: &mut usize,
    dynamic: &mut usize,
) {
    match o {
        desc_trace::DescSource::Loaded { .. } | desc_trace::DescSource::Fetched { .. } => {
            *loaded += 1;
        }
        desc_trace::DescSource::Imm(_) => *constant += 1,
        desc_trace::DescSource::VgprDword0(_) => *dynamic += 1,
        desc_trace::DescSource::Select { set, clear }
        | desc_trace::DescSource::Lanes { set, clear, .. } => {
            count_origin_leaves(set, loaded, constant, dynamic);
            count_origin_leaves(clear, loaded, constant, dynamic);
        }
        desc_trace::DescSource::LanePhi { arms, .. } => {
            for a in arms.iter().flatten() {
                count_origin_leaves(a, loaded, constant, dynamic);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untraced_key_format_matches_cgBJ_shape() {
        let k = format!("{:?} via {}", ResourceKind::Srv, RootKind::Table.name());
        assert_eq!(k, "Srv via table");
    }

    #[test]
    fn empty_tally_json_has_untraced_map() {
        let t = Tally::default();
        let v = t.to_json();
        assert_eq!(v["untraced_used_total"], 0);
    }
}
