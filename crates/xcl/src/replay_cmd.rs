//! `xcl replay` – inspect / edit / compare an `XCL_RECORD` capture and optionally run a title.
//!
//! Same-process list re-issue only: the capture must be replayed while the title is live with
//! still-valid GPU pointers. Offline `--listing` / `--edit` / `--pixel` / `--diverge` operate on
//! `XCLFRAME` wire blobs without a GPU. This is not a full cross-process VICFRAME player (COM
//! command-list handles cannot be rebuilt from payload bytes alone).

use std::path::{Path, PathBuf};

use crate::hle::d3d12x::{
    cli_diverge, cli_edit, cli_listing, cli_pixel, frame_replay_inspect, parse_point, parse_range,
    parse_wh, EditOpts,
};

/// Summarise a capture folder, run offline tools, apply replay env knobs, and optionally `xcl run`.
pub fn run(folder: &Path, args: &[String]) -> i32 {
    let mut loops_hint = 1u32;
    let mut wire = false;
    let mut execute = true;
    let mut fences = false;
    let mut present = false;
    let mut contents = true;
    let mut json = false;
    let mut show_notes = false;
    let mut listing = false;
    let mut edit = false;
    let mut diverge = false;
    let mut write_diverge_report = true;
    let mut at = 0u64;
    let mut frame_filter: Option<u64> = None;
    let mut run_target: Option<PathBuf> = None;
    let mut title_args: Vec<String> = Vec::new();
    let mut edit_opts = EditOpts::default();
    let mut out_dir: Option<PathBuf> = None;
    let mut points: Vec<(u32, u32)> = Vec::new();
    let mut picture: Option<(u32, u32)> = None;
    let mut compare_dir: Option<PathBuf> = None;
    let mut against_dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--wire" => wire = true,
            "--no-execute" => execute = false,
            "--fences" => fences = true,
            "--present" => present = true,
            "--no-contents" => contents = false,
            "--json" => json = true,
            "--notes" => show_notes = true,
            "--listing" | "--list-ops" => listing = true,
            "--edit" => edit = true,
            "--skip-draws" => {
                edit = true;
                edit_opts.skip_draws = true;
            }
            "--skip-dispatches" => {
                edit = true;
                edit_opts.skip_dispatches = true;
            }
            "--meta-only" | "--barriers-only" => {
                edit = true;
                edit_opts.meta_only = true;
            }
            "--diverge" => diverge = true,
            "--no-diverge-report" => write_diverge_report = false,
            "--at" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --at needs a value");
                    return 2;
                };
                at = match v.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("xcl replay: invalid --at {v}");
                        return 2;
                    }
                };
            }
            "--frame" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --frame needs a value");
                    return 2;
                };
                frame_filter = match v.parse() {
                    Ok(n) => Some(n),
                    Err(_) => {
                        eprintln!("xcl replay: invalid --frame {v}");
                        return 2;
                    }
                };
            }
            "--loops" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --loops needs a value");
                    return 2;
                };
                loops_hint = match v.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("xcl replay: invalid --loops {v}");
                        return 2;
                    }
                };
            }
            "--lists" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --lists needs A..B");
                    return 2;
                };
                match parse_range(v) {
                    Ok(r) => {
                        edit = true;
                        edit_opts.only_lists = Some(r);
                    }
                    Err(e) => {
                        eprintln!("xcl replay: {e}");
                        return 2;
                    }
                }
            }
            "--out" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --out needs a directory");
                    return 2;
                };
                edit = true;
                out_dir = Some(PathBuf::from(v));
            }
            "--pixel" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --pixel needs x,y");
                    return 2;
                };
                match parse_point(v) {
                    Ok(p) => points.push(p),
                    Err(e) => {
                        eprintln!("xcl replay: {e}");
                        return 2;
                    }
                }
            }
            "--of" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --of needs WxH");
                    return 2;
                };
                match parse_wh(v) {
                    Ok(wh) => picture = Some(wh),
                    Err(e) => {
                        eprintln!("xcl replay: {e}");
                        return 2;
                    }
                }
            }
            "--compare" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --compare needs a capture directory");
                    return 2;
                };
                compare_dir = Some(PathBuf::from(v));
            }
            "--against" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --against needs a capture directory");
                    return 2;
                };
                against_dir = Some(PathBuf::from(v));
                diverge = true;
            }
            "--run" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("xcl replay: --run needs a title path");
                    return 2;
                };
                run_target = Some(PathBuf::from(v));
            }
            "--" => {
                title_args.extend(args[i + 1..].iter().cloned());
                break;
            }
            other if other.starts_with('-') => {
                eprintln!("xcl replay: unknown option {other}");
                return 2;
            }
            other => {
                eprintln!("xcl replay: unexpected argument {other}");
                return 2;
            }
        }
        i += 1;
    }

    if !folder.is_dir() {
        eprintln!("xcl replay: {} is not a directory", folder.display());
        return 2;
    }

    let offline_only =
        listing || edit || diverge || show_notes || !points.is_empty() || compare_dir.is_some();

    if !offline_only || run_target.is_some() {
        match frame_replay_inspect(folder, json && !offline_only, frame_filter) {
            Ok(s) => {
                if !offline_only || run_target.is_some() {
                    println!("{s}");
                }
            }
            Err(e) => {
                // Allow tooling on wire-only folders that lack index.jsonl when not --run.
                if run_target.is_some() || !offline_only {
                    eprintln!("xcl replay: {e}");
                    return 3;
                }
            }
        }
    }

    if show_notes {
        match frame_notes(folder, frame_filter) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("xcl replay notes: {e}");
                return 3;
            }
        }
    }

    if listing {
        match cli_listing(folder, frame_filter, json) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("xcl replay listing: {e}");
                return 3;
            }
        }
    }

    if edit {
        let out = out_dir.as_deref();
        match cli_edit(folder, &edit_opts, out, frame_filter, json) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("xcl replay edit: {e}");
                return 3;
            }
        }
    }

    if !points.is_empty() || compare_dir.is_some() {
        match cli_pixel(
            folder,
            &points,
            picture,
            compare_dir.as_deref(),
            frame_filter,
            json,
        ) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("xcl replay pixel: {e}");
                return 3;
            }
        }
    }

    if diverge {
        match cli_diverge(
            folder,
            against_dir.as_deref().or(compare_dir.as_deref()),
            frame_filter,
            write_diverge_report,
            json,
        ) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("xcl replay diverge: {e}");
                return 3;
            }
        }
    }

    if run_target.is_none() {
        if !json && !offline_only {
            println!(
                "\nInspect only. Offline tools:\n  xcl replay \"{dir}\" --listing\n  xcl replay \"{dir}\" --edit --skip-draws [--out edited]\n  xcl replay \"{dir}\" --pixel 100,200 --of 1280x720\n  xcl replay \"{dir}\" --diverge --against <other-capture>\nIn-process replay:\n  xcl replay \"{dir}\" --run <game> [--wire] [--fences] [--present] [--at N]",
                dir = folder.display()
            );
            println!("  (sets XCL_REPLAY; same-process GPU pointers required — not a cross-process player)");
        }
        let _ = loops_hint;
        return 0;
    }

    std::env::set_var("XCL_REPLAY", folder);
    if wire {
        std::env::set_var("XCL_REPLAY_WIRE", "1");
    }
    if !execute {
        std::env::set_var("XCL_REPLAY_EXECUTE", "0");
    }
    if fences {
        std::env::set_var("XCL_REPLAY_FENCES", "1");
    }
    if present {
        std::env::set_var("XCL_REPLAY_PRESENT", "1");
    }
    if wire && !contents {
        std::env::set_var("XCL_REPLAY_CONTENTS", "0");
    }
    if at > 0 {
        std::env::set_var("XCL_REPLAY_AT", at.to_string());
    }
    if loops_hint > 1 {
        eprintln!(
            "xcl replay: note: --loops={loops_hint} is a hint only; in-process replay advances one recorded frame per present"
        );
    }

    let target = run_target.unwrap();
    println!(
        "xcl run \"{}\" with XCL_REPLAY={} (wire={wire}, execute={execute}, fences={fences}, present={present}, at={at})",
        target.display(),
        folder.display()
    );
    crate::launcher::run(&target, &title_args)
}

fn frame_notes(dir: &Path, frame_filter: Option<u64>) -> Result<String, String> {
    let frames_dir = dir.join("frames");
    if !frames_dir.is_dir() {
        return Err("no frames/ directory".into());
    }
    let mut out = String::new();
    let mut n = 0u64;
    loop {
        n += 1;
        let notes = frames_dir.join(format!("frame_{n:04}.notes.txt"));
        if !notes.is_file() {
            break;
        }
        if frame_filter.is_some_and(|f| f != n) {
            continue;
        }
        let text = std::fs::read_to_string(&notes).map_err(|e| e.to_string())?;
        out.push_str(&format!("--- frame {n} ---\n{text}\n"));
    }
    if out.is_empty() {
        out.push_str("no frames/frame_NNNN.notes.txt found (record with XCL_RECORD_WIRE)\n");
    }
    Ok(out)
}
