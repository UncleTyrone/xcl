//! `xcl replay` – inspect an `XCL_RECORD` capture and optionally run a title with `XCL_REPLAY`.
//!
//! Same-process list re-issue only: the capture must be replayed while the title is live with
//! still-valid GPU pointers. This is the CLI entry for the existing `frame_replay` path, not a
//! full offline VICFRAME player (COM command-list handles cannot be rebuilt cross-process).

use std::path::{Path, PathBuf};

/// Summarise a capture folder, apply replay env knobs, and optionally `xcl run` a title.
pub fn run(folder: &Path, args: &[String]) -> i32 {
    let mut loops_hint = 1u32;
    let mut wire = false;
    let mut execute = true;
    let mut fences = false;
    let mut present = false;
    let mut contents = true;
    let mut json = false;
    let mut at = 0u64;
    let mut frame_filter: Option<u64> = None;
    let mut run_target: Option<PathBuf> = None;
    let mut title_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--wire" => wire = true,
            "--no-execute" => execute = false,
            "--fences" => fences = true,
            "--present" => present = true,
            "--no-contents" => contents = false,
            "--json" => json = true,
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

    match crate::hle::d3d12x::frame_replay_inspect(folder, json, frame_filter) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("xcl replay: {e}");
            return 3;
        }
    }

    if run_target.is_none() {
        if !json {
            println!(
                "\nInspect only. To replay in-process during a title run:\n  xcl replay \"{dir}\" --run <game> [--wire] [--fences] [--present] [--at N]",
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
