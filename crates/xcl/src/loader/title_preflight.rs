//! Static-import check before the title entry: game-shipped DLLs that are not beside
//! the executable fail early (retail game #930 during System Init at +0x2237178 when e.g.
//! XboxBvhBuilder.dll or Bink2Scarlett.dll are missing).

use std::path::Path;

use crate::hle::{registry, Route};
use crate::pe::{Layout, Pe};
use crate::resolver::{find_game_dll, system_module};
use crate::util::dll_key;

/// On by default; `XCL_TITLE_DLL_PREFLIGHT=0` skips the check.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_TITLE_DLL_PREFLIGHT").is_none_or(|v| v != "0"))
}

/// Log and continue instead of bailing (CGBJ `--unimplemented zero` analogue).
pub fn lenient() -> bool {
    std::env::var_os("XCL_TITLE_DLL_LENIENT").is_some()
}

/// DLLs HLE / Windows routing satisfy — not required from the title folder.
fn must_come_from_title_folder(dll: &str) -> bool {
    let key = dll_key(dll);
    match registry().route(&key) {
        Some(Route::Hle(_) | Route::System | Route::Stub) => false,
        None => system_module(&key).is_none(),
    }
}

/// Imported DLL names that must be present beside the executable but are not (each once, case-insensitive).
pub fn absent_title_dlls(exe: &Path) -> Result<Vec<String>, String> {
    let data = std::fs::read(exe).map_err(|e| format!("reading {}: {e}", exe.display()))?;
    let pe = Pe::parse(&data, Layout::File).map_err(|e| format!("{}: {e}", exe.display()))?;
    let mut all = pe.imports().map_err(|e| format!("imports: {e}"))?;
    all.extend(pe.delay_imports().unwrap_or_default());
    let mut seen = std::collections::HashSet::new();
    let mut absent = Vec::new();
    for d in &all {
        let key = dll_key(&d.dll);
        if !seen.insert(key) || !must_come_from_title_folder(&d.dll) {
            continue;
        }
        if find_game_dll(&d.dll).is_none() && !absent.iter().any(|a: &String| a.eq_ignore_ascii_case(&d.dll)) {
            absent.push(d.dll.clone());
        }
    }
    Ok(absent)
}

/// Run before `load_main` / entry; returns an error unless lenient or preflight is off.
pub fn check_or_bail(exe: &Path, title_dir: &Path) -> Result<(), String> {
    if !enabled() {
        crate::xlog_info!("title DLL preflight: off (XCL_TITLE_DLL_PREFLIGHT=0)");
        return Ok(());
    }
    let absent = absent_title_dlls(exe)?;
    if absent.is_empty() {
        crate::xlog_info!("title DLL preflight: every imported game DLL is beside the executable");
        return Ok(());
    }
    let message = format!(
        "{} not in {}, and neither Windows nor XCL provides {}: the package is incomplete; copy every file of the package next to the executable",
        absent.join(", "),
        title_dir.display(),
        if absent.len() == 1 { "it" } else { "them" },
    );
    crate::xlog_error!("missing title DLLs: {message}");
    if lenient() {
        crate::xlog_warn!(
            "XCL_TITLE_DLL_LENIENT=1: continuing anyway (missing imports may stub; retail game often stops at System Init #930 without a clear message)"
        );
        return Ok(());
    }
    Err(format!(
        "the title's own DLLs are missing: {message} (XCL_TITLE_DLL_LENIENT=1 to run anyway, XCL_TITLE_DLL_PREFLIGHT=0 to skip)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_game_dlls_are_named_once() {
        fn wanted_sim(dll: &str) -> bool {
            !["KERNEL32.dll", "d3d12_xs.dll"]
                .iter()
                .any(|h| h.eq_ignore_ascii_case(dll))
        }
        let imported = [
            "KERNEL32.dll",
            "d3d12_xs.dll",
            "XboxBvhBuilder.dll",
            "MSVCP140.dll",
            "Bink2Scarlett.dll",
            "xboxbvhbuilder.dll",
        ];
        let mut absent: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for &dll in &imported {
            let key = crate::util::dll_key(dll);
            if !seen.insert(key) || !wanted_sim(dll) {
                continue;
            }
            let on_disk = dll.eq_ignore_ascii_case("msvcp140.dll");
            if !on_disk && !absent.iter().any(|a| a.eq_ignore_ascii_case(dll)) {
                absent.push(dll.to_owned());
            }
        }
        assert_eq!(absent, ["XboxBvhBuilder.dll", "Bink2Scarlett.dll"]);
    }
}