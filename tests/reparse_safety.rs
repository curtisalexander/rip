//! Reparse-point safety: ripping a tree that contains a symlink/junction must
//! delete the *link*, never traverse it and destroy the data it points at.
//!
//! This is the single most dangerous failure mode for a fast recursive deleter
//! (think: a junction in `.venv` pointing at your home directory). The test runs
//! the real `rip` binary end-to-end. On Unix it uses a directory symlink; on
//! Windows it uses a junction (`mklink /J`), which — unlike a Windows symlink —
//! requires no administrator or Developer Mode, so it runs in plain CI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workdir(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(tag);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
fn make_dir_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn make_dir_link(target: &Path, link: &Path) {
    // `mklink /J` creates a junction and needs no elevation, unlike symlink_dir.
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("failed to run mklink");
    assert!(status.success(), "mklink /J did not succeed");
}

/// Try to create a Windows *directory symbolic link* (`mklink /D`), which —
/// unlike a junction — needs elevation or Developer Mode. Returns `false` if it
/// couldn't be created so the caller can skip rather than fail spuriously.
#[cfg(windows)]
fn try_make_dir_symlink(target: &Path, link: &Path) -> bool {
    let made = Command::new("cmd")
        .args(["/C", "mklink", "/D"])
        .arg(link)
        .arg(target)
        .status();
    matches!(made, Ok(s) if s.success()) && link.exists()
}

fn rip_force(path: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_rip"))
        .arg("--force")
        .arg(path)
        .status()
        .expect("failed to run rip");
    assert!(status.success(), "rip exited unsuccessfully");
}

/// A link *inside* the ripped tree must not let rip reach outside it.
#[test]
fn rip_deletes_the_link_not_its_target() {
    let work = workdir("reparse_link_not_target");

    // External target tree, a SIBLING of (not under) the tree we will rip.
    let external = work.join("external_target");
    fs::create_dir_all(&external).unwrap();
    let canary = external.join("canary.txt");
    fs::write(&canary, b"DO NOT DELETE").unwrap();

    // The tree we will rip, containing a link pointing at `external`.
    let root = work.join("rip_root");
    fs::create_dir_all(root.join("normal_subdir")).unwrap();
    fs::write(root.join("normal_subdir").join("file.txt"), b"x").unwrap();
    make_dir_link(&external, &root.join("link_to_external"));

    // Sanity: the link really does resolve to the external canary first.
    assert!(
        root.join("link_to_external").join("canary.txt").exists(),
        "test setup is wrong — link does not resolve to the canary"
    );

    rip_force(&root);

    // The ripped tree (and the link within it) must be gone...
    assert!(!root.exists(), "rip_root should have been deleted");
    // ...but the external target and its canary MUST survive untouched.
    assert!(
        external.exists(),
        "external target directory was destroyed through the link!"
    );
    assert!(
        canary.exists(),
        "canary file was destroyed by traversing the link!"
    );
    assert_eq!(fs::read(&canary).unwrap(), b"DO NOT DELETE");
}

/// Ripping a link *directly* must remove the link and preserve its target.
#[test]
fn ripping_a_link_directly_preserves_target() {
    let work = workdir("reparse_link_direct");

    let external = work.join("external_target");
    fs::create_dir_all(&external).unwrap();
    let canary = external.join("canary.txt");
    fs::write(&canary, b"KEEP").unwrap();

    let link = work.join("the_link");
    make_dir_link(&external, &link);

    rip_force(&link);

    assert!(!link.exists(), "the link itself should have been removed");
    assert!(external.exists(), "target directory must survive");
    assert!(canary.exists(), "target canary must survive");
    assert_eq!(fs::read(&canary).unwrap(), b"KEEP");
}

/// The same guarantee as `rip_deletes_the_link_not_its_target`, but for a
/// Windows *directory symlink* (`mklink /D`) rather than a junction. Junctions
/// (`IO_REPARSE_TAG_MOUNT_POINT`) and symlinks (`IO_REPARSE_TAG_SYMLINK`) are
/// distinct reparse kinds; the no-follow guarantee must hold for both, so we
/// cover the symlink explicitly instead of inferring it from the junction case.
/// Skipped (not failed) when the environment can't create one.
#[cfg(windows)]
#[test]
fn rip_deletes_a_directory_symlink_not_its_target() {
    let work = workdir("reparse_dir_symlink");

    let external = work.join("external_target");
    fs::create_dir_all(&external).unwrap();
    let canary = external.join("canary.txt");
    fs::write(&canary, b"DO NOT DELETE").unwrap();

    let root = work.join("rip_root");
    fs::create_dir_all(root.join("normal_subdir")).unwrap();
    fs::write(root.join("normal_subdir").join("file.txt"), b"x").unwrap();

    let link = root.join("symlink_to_external");
    if !try_make_dir_symlink(&external, &link) {
        eprintln!(
            "skipping rip_deletes_a_directory_symlink_not_its_target: \
             could not create a directory symlink (needs Developer Mode or elevation)"
        );
        return;
    }

    // Sanity: the symlink really does resolve to the external canary first.
    assert!(
        link.join("canary.txt").exists(),
        "test setup is wrong — symlink does not resolve to the canary"
    );

    rip_force(&root);

    assert!(!root.exists(), "rip_root should have been deleted");
    assert!(
        external.exists(),
        "external target directory was destroyed through the symlink!"
    );
    assert!(
        canary.exists(),
        "canary file was destroyed by traversing the symlink!"
    );
    assert_eq!(fs::read(&canary).unwrap(), b"DO NOT DELETE");
}
