//! Rebuild when any embedded catalog file changes: `include_dir!`
//! inlines `registry/` at macro expansion, and the pinned 1.83 toolchain
//! lacks `proc_macro::tracked_path`, so without these directives an
//! edit to `registry/*.json` would silently ship the stale catalog.

use std::path::Path;

fn main() {
    let registry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry");
    emit(&registry);
}

fn emit(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            emit(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
