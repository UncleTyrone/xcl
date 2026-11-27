//! BCPack env latches (`XCL_BCPACK_*`, legacy `VICTORY_BCPACK_*`; CGBJ `victory-formats` parity).

use std::path::PathBuf;
use std::sync::OnceLock;

use super::load;
use super::victory;

fn first_path(names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .find_map(|n| std::env::var_os(n).filter(|v| !v.is_empty()).map(PathBuf::from))
}

/// Optional directory of `<name>.bc<n>` chunks with `<name>.out` or `<name>.vic<n>` beside them
/// (`XCL_BCPACK_CASES`, `VICTORY_BCPACK_CASES`, or `XCL_SAMPLE_BCPACK` for the retail parity test).
pub fn cases_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        first_path(&["XCL_BCPACK_CASES", "VICTORY_BCPACK_CASES", "XCL_SAMPLE_BCPACK"])
    })
    .clone()
}

/// Reference-chunk directory only (`XCL_BCPACK_CASES` / `VICTORY_BCPACK_CASES`; CGBJ `reference_chunks`).
pub fn reference_cases_dir() -> Option<PathBuf> {
    first_path(&["XCL_BCPACK_CASES", "VICTORY_BCPACK_CASES"])
}

/// Optional path to a `bcpack-tables.bin` (`XCL_BCPACK_TABLES`, legacy `VICTORY_BCPACK_TABLES`).
/// Unset uses the built-in tables from `tables.rs` (CGBJ default in production).
pub fn tables_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| first_path(&["XCL_BCPACK_TABLES", "VICTORY_BCPACK_TABLES"])).clone()
}

/// Tables for [`super::decode`]: built-in by default; overridden when `tables_path()` is set.
pub(crate) fn tables_for_decode() -> &'static victory::Tables {
    static T: OnceLock<&'static victory::Tables> = OnceLock::new();
    T.get_or_init(|| match tables_path() {
        None => victory::Tables::builtin(),
        Some(ref path) => match load::tables_from_file(path) {
            Ok(t) => {
                crate::xlog_info!("BCPack: tables loaded from {}", path.display());
                t
            }
            Err(e) => {
                crate::xlog_info!("BCPack: {:?}: {e}; built-in tables", path);
                victory::Tables::builtin()
            }
        },
    })
}
