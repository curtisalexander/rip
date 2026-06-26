//! Long-path safety: rip must delete trees whose paths exceed the legacy
//! Windows `MAX_PATH` (260 characters). Deep `node_modules`/`.git` trees blow
//! past that constantly, and rip's raw `CreateFileW` calls don't get std's
//! automatic long-path handling — so it prefixes paths with `\\?\` itself.
//!
//! On Unix this is just a deep-tree deletion sanity check; on Windows it
//! specifically exercises the verbatim-prefix path.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workdir(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(tag);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn rip_force(path: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_rip"))
        .arg("--force")
        .arg(path)
        .status()
        .expect("failed to run rip");
    assert!(status.success(), "rip exited unsuccessfully");
}

#[test]
fn rips_paths_longer_than_legacy_max_path() {
    let work = workdir("long_path");

    // Build a tree whose deepest path comfortably exceeds 260 characters.
    let segment = |i: usize| format!("segment_{i:02}_{}", "x".repeat(48));
    let root = work.join(segment(0));
    let mut deep = root.clone();
    for i in 1..8 {
        deep = deep.join(segment(i));
    }
    fs::create_dir_all(&deep).unwrap();
    let file = deep.join("buried_file.txt");
    fs::write(&file, b"deep").unwrap();

    // Confirm the test actually crosses the limit it is meant to guard.
    assert!(
        file.as_os_str().len() > 260,
        "test path is only {} chars — not past MAX_PATH",
        file.as_os_str().len()
    );
    assert!(file.exists(), "test setup failed to create the deep file");

    rip_force(&root);

    assert!(!root.exists(), "deep tree should have been deleted");
}
