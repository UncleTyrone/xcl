//! `xcl census` – aggregate PE imports under paths and score HLE / host / stub coverage.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::pe::{Layout, Pe};
use crate::resolver::{self, Source};

#[derive(Default)]
struct DllStats {
    functions: BTreeSet<String>,
    hle: usize,
    host: usize,
    game: usize,
    stub: usize,
    images: BTreeSet<PathBuf>,
}

/// Walk `paths` (files or directories) for PE images and print import coverage.
///
/// `strict`: count Windows pass-through as uncovered (same role as CGBJ `--strict`).
/// `list_missing`: print every stub `dll!function`, most used first.
pub fn run(paths: &[PathBuf], strict: bool, list_missing: bool) -> i32 {
    if paths.is_empty() {
        eprintln!("xcl census: pass one or more files or folders (e.g. the game package root)");
        return 2;
    }

    let mut images = 0usize;
    let mut seen = BTreeSet::new();
    let mut failed = Vec::new();
    let mut dlls: BTreeMap<String, DllStats> = BTreeMap::new();
    let mut missing: BTreeMap<(String, String), BTreeSet<PathBuf>> = BTreeMap::new();

    for root in paths {
        if !root.exists() {
            failed.push(format!("{}: path does not exist", root.display()));
            continue;
        }
        walk_pe(root, &mut |path| {
            let identity = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if !seen.insert(identity) {
                return;
            }
            let data = match std::fs::read(path) {
                Ok(d) => d,
                Err(e) => {
                    failed.push(format!("{}: {e}", path.display()));
                    return;
                }
            };
            let pe = match Pe::parse(&data, Layout::File) {
                Ok(p) => p,
                Err(e) => {
                    failed.push(format!("{}: {e}", path.display()));
                    return;
                }
            };
            if let Some(dir) = path.parent() {
                resolver::set_search_dirs(vec![dir.to_path_buf()]);
            }
            images += 1;
            let mut all = pe.imports().unwrap_or_default();
            all.extend(pe.delay_imports().unwrap_or_default());
            for d in &all {
                let key = crate::util::dll_key(&d.dll);
                let stats = dlls.entry(key.clone()).or_default();
                stats.images.insert(path.to_path_buf());
                for e in &d.entries {
                    let name = e.what.to_string();
                    stats.functions.insert(name.clone());
                    let src = resolver::classify(&d.dll, &e.what);
                    match classify_bucket(&src, strict) {
                        Bucket::Hle => stats.hle += 1,
                        Bucket::Host => stats.host += 1,
                        Bucket::Game => stats.game += 1,
                        Bucket::Missing => {
                            stats.stub += 1;
                            missing
                                .entry((key.clone(), name))
                                .or_default()
                                .insert(path.to_path_buf());
                        }
                    }
                }
            }
        });
    }

    println!("{images} images scanned, {} unreadable/skipped", failed.len());
    for error in &failed {
        println!("  skipped {error}");
    }
    println!(
        "\n{:<56} {:>6} {:>6} {:>6} {:>6} {:>8} {:>7}",
        "dll", "funcs", "hle", "host", "game", "missing", "images"
    );
    let (mut f, mut h, mut o, mut g, mut m) = (0usize, 0, 0, 0, 0);
    for (dll, s) in &dlls {
        println!(
            "{dll:<56} {:>6} {:>6} {:>6} {:>6} {:>8} {:>7}",
            s.functions.len(),
            s.hle,
            s.host,
            s.game,
            s.stub,
            s.images.len()
        );
        f += s.functions.len();
        h += s.hle;
        o += s.host;
        g += s.game;
        m += s.stub;
    }
    println!(
        "{:<56} {:>6} {:>6} {:>6} {:>6} {:>8}",
        "TOTAL (symbol refs)", f, h, o, g, m
    );

    if list_missing {
        let mut rows: Vec<_> = missing.into_iter().collect();
        rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        println!("\nmissing (stub) imports, most used first:");
        for ((dll, fun), imgs) in rows {
            println!("  {dll}!{fun}  ({} image{})", imgs.len(), if imgs.len() == 1 { "" } else { "s" });
        }
    }
    0
}

enum Bucket {
    Hle,
    Host,
    Game,
    Missing,
}

fn classify_bucket(src: &Source, strict: bool) -> Bucket {
    match src {
        Source::Hle(_) | Source::HleOther(_) => Bucket::Hle,
        Source::System(_) if strict => Bucket::Missing,
        Source::System(_) => Bucket::Host,
        Source::GameDll(_) => Bucket::Game,
        Source::Stub => Bucket::Missing,
    }
}

fn walk_pe(root: &Path, visit: &mut dyn FnMut(&Path)) {
    if root.is_file() {
        if looks_like_pe(root) {
            visit(root);
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip bulky non-PE trees when walking a full game package.
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let lower = name.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "cache" | ".git" | "node_modules" | "shadercache" | "crashdumps"
                ) {
                    continue;
                }
            }
            walk_pe(&path, visit);
        } else if looks_like_pe(&path) {
            visit(&path);
        }
    }
}

fn looks_like_pe(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let e = ext.to_ascii_lowercase();
            e == "exe" || e == "dll" || e == "xex" || e == "mui"
        }
        None => false,
    }
}
