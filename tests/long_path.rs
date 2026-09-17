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

#[test]
fn mixed_depth_buckets_count_and_delete_every_directory() {
    let work = workdir("depth_buckets");
    let root = work.join("victim");
    // Uneven branches, empty directories, a hidden directory and a read-only
    // file: root-first deletion or counting buckets instead of dirs must fail.
    for dir in ["a/b/c", "a/empty", "sibling", ".hidden"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in ["top.txt", "a/b/c/deep.txt", ".hidden/readonly.txt"] {
        fs::write(root.join(file), b"keep until deletion").unwrap();
    }
    let readonly = root.join(".hidden/readonly.txt");
    let mut permissions = fs::metadata(&readonly).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&readonly, permissions).unwrap();

    for dry_run in [true, false] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rip"));
        // Walking and deleting must also complete with a single worker.
        command.current_dir(&work).args(["-f", "-j", "1"]);
        if dry_run {
            command.arg("-n");
        }
        let output = command.arg("./victim").output().unwrap();
        assert!(output.status.success());
        let summary = String::from_utf8_lossy(&output.stderr);
        assert!(summary.contains("3 files, 7 dirs"), "{summary}");
        assert!(summary.contains("(0 errors)"), "{summary}");
        assert_eq!(root.exists(), dry_run);
        if dry_run {
            assert_eq!(fs::read(&readonly).unwrap(), b"keep until deletion");
        }
    }
}
