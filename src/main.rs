//! RIP — rip through a directory tree and delete it as fast as possible.
//!
//! Strategy:
//!   1. Walk the tree in parallel (jwalk) to collect every file and directory.
//!   2. Delete all files in parallel (rayon), clearing read-only flags first.
//!   3. Remove directories deepest-first (a dir can only be removed once empty).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rayon::prelude::*;

mod platform;
mod progress;
mod ui;

#[derive(Parser, Debug)]
#[command(
    name = "rip",
    version,
    about = "Rip through directories and delete them as fast as possible",
    long_about = "RIP deletes folders FAST by walking and deleting in parallel.\n\
                  Great for the things that make Windows crawl: .git, .venv, node_modules."
)]
struct Args {
    /// Paths to delete (files or directories).
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Show what would be deleted without deleting anything.
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Move to the Recycle Bin / Trash instead of permanently deleting (recoverable).
    #[arg(short = 't', long)]
    trash: bool,

    /// Skip all warnings and confirmation — just rip it out.
    #[arg(short = 'f', long, visible_alias = "yes", visible_short_alias = 'y')]
    force: bool,

    /// Number of worker threads (default: number of logical CPUs).
    #[arg(short = 'j', long)]
    threads: Option<usize>,

    /// Print each deleted path.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(n) = args.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .context("failed to configure thread pool")?;
    }

    // Confirm the worker count on demand. With no `-j`, this reports rayon's
    // default global pool — one thread per logical CPU — which is the mapping
    // a user staring at Task Manager can't otherwise see.
    if args.verbose {
        eprintln!("rip: {} worker threads", rayon::current_num_threads());
    }

    // Validate targets up front. Use `symlink_metadata` rather than `exists()`
    // so a dangling symlink — a real entry the user may well want gone — is
    // accepted instead of wrongly rejected as "does not exist".
    for p in &args.paths {
        // Guard: never rip a filesystem/drive/UNC root. A bare `rip C:\` (or
        // `/`, or `\\server\share`) is almost always an accident — a stray
        // trailing argument, an unexpanded variable, a misplaced slash — and
        // there is no legitimate reason to recursively delete an entire volume.
        // Refused unconditionally, *before* any confirmation or --force, so even
        // `rip -f /` stops here instead of walking the disk.
        if is_root(p) {
            bail!(
                "refusing to rip a filesystem root: {} — this is almost \
                 certainly a mistake. rip is for throwaway subtrees \
                 (.git, .venv, node_modules, target/), not whole volumes.",
                p.display()
            );
        }
        if p.symlink_metadata().is_err() {
            bail!("path does not exist: {}", p.display());
        }
    }

    // Warn loudly and confirm — unless forced or just doing a dry run.
    if !args.force && !args.dry_run {
        let mode = if args.trash {
            ui::Mode::Trash
        } else {
            ui::Mode::Rip
        };
        if !ui::confirm(&args.paths, mode) {
            eprintln!("aborted.");
            return Ok(());
        }
    }

    let start = Instant::now();

    if args.trash {
        return trash_paths(&args, start);
    }

    let show_progress = progress::enabled(args.verbose, args.dry_run);
    let stats = Stats::default();
    for path in &args.paths {
        if let Err(e) = rip_path(path, &args, &stats, show_progress) {
            eprintln!("error ripping {}: {e:#}", path.display());
        }
    }

    let elapsed = start.elapsed();
    let files = stats.files.load(Ordering::Relaxed);
    let dirs = stats.dirs.load(Ordering::Relaxed);
    let errors = stats.errors.load(Ordering::Relaxed);
    let verb = if args.dry_run {
        "would delete"
    } else {
        "deleted"
    };

    eprintln!(
        "\nRIP: {verb} {files} files, {dirs} dirs in {:.2}s ({errors} errors)",
        elapsed.as_secs_f64(),
    );
    Ok(())
}

/// Recycle-bin mode: hand each top-level path to the OS trash as a unit.
fn trash_paths(args: &Args, start: Instant) -> Result<()> {
    let mut moved = 0u64;
    let mut errors = 0u64;

    for path in &args.paths {
        if args.dry_run {
            if args.verbose {
                println!("would trash {}", path.display());
            }
            moved += 1;
            continue;
        }
        match trash::delete(path) {
            Ok(()) => {
                if args.verbose {
                    println!("{}", path.display());
                }
                moved += 1;
            }
            Err(e) => {
                eprintln!("error: trash {}: {e}", path.display());
                errors += 1;
            }
        }
    }

    let verb = if args.dry_run {
        "would trash"
    } else {
        "trashed"
    };
    eprintln!(
        "\nRIP: {verb} {moved} path(s) in {:.2}s ({errors} errors)",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

/// True if `path` refers to a filesystem root: a Unix `/`, a Windows drive
/// root (`C:\`), or a UNC share root (`\\server\share`).
///
/// The path is fully qualified *lexically* first (`std::path::absolute` — no
/// filesystem access, so it resolves `.`/`..`/drive-relative forms without ever
/// following a symlink), then we ask whether it has a parent. A root is the only
/// thing that doesn't, which makes this a precise, allocation-light test that
/// also catches the disguised forms (`C:\.`, `C:\foo\..`, a trailing slash).
fn is_root(path: &Path) -> bool {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    abs.parent().is_none()
}

#[derive(Default)]
struct Stats {
    files: AtomicU64,
    dirs: AtomicU64,
    errors: AtomicU64,
}

fn rip_path(root: &Path, args: &Args, stats: &Stats, show_progress: bool) -> Result<()> {
    let meta = std::fs::symlink_metadata(root)
        .with_context(|| format!("cannot stat {}", root.display()))?;

    // A single file (or symlink): just remove it.
    if !meta.is_dir() {
        if args.dry_run {
            if args.verbose {
                println!("would delete {}", root.display());
            }
            stats.files.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        platform::remove_file(root).with_context(|| format!("remove {}", root.display()))?;
        if args.verbose {
            println!("{}", root.display());
        }
        stats.files.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }

    // Directory: collect everything, deepest paths first.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();

    let scan = progress::scanner(show_progress);
    let scanning = !scan.is_hidden();

    // SAFETY: `follow_links(false)` is critical — it ensures we never descend
    // through a symlink or junction into a tree outside `root`. A reparse point
    // is yielded as a leaf entry (is_dir() == false) and deleted as a link, so
    // we remove the link itself, never the data it points at. The reparse-safety
    // integration test guards this guarantee.
    for (i, entry) in jwalk::WalkDir::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                scan.suspend(|| eprintln!("walk error: {e}"));
                stats.errors.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let path = entry.path();
        // Treat symlinks as files — never follow them into other trees.
        if entry.file_type().is_dir() {
            dirs.push(path);
        } else {
            files.push(path);
        }
        // Refresh the count periodically — but never on the first entry, so a
        // small tree (which never reaches the interval) shows no spinner at all.
        if scanning && i > 0 && i % progress::SCAN_REFRESH == 0 {
            scan.set_position((files.len() + dirs.len()) as u64);
        }
    }
    scan.finish_and_clear();

    let total = (files.len() + dirs.len()) as u64;
    let bar = progress::deleter(show_progress, total);
    // Capture visibility once so the hot loop never touches the bar (and never
    // takes its lock) when there's nothing to draw.
    let tracking = !bar.is_hidden();

    // Delete files in parallel.
    files.par_iter().for_each(|f| {
        if args.dry_run {
            if args.verbose {
                println!("would delete {}", f.display());
            }
            stats.files.fetch_add(1, Ordering::Relaxed);
            return;
        }
        match platform::remove_file(f) {
            Ok(()) => {
                if args.verbose {
                    println!("{}", f.display());
                }
                stats.files.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                bar.suspend(|| eprintln!("error: remove {}: {e:#}", f.display()));
                stats.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        if tracking {
            bar.inc(1);
        }
    });

    // Remove directories deepest-first: a directory can only be removed once
    // it's empty, so every child must go before its parent. For paths under one
    // root, more path components always means deeper, so removing in batches of
    // descending component count keeps children ahead of parents. Crucially,
    // every directory *within* a single batch is at the same depth — so none is
    // an ancestor of another, and the whole batch is safe to remove in parallel.
    // Pair each path with its depth once, up front, so neither the sort nor the
    // batching recomputes it.
    let mut dirs: Vec<(usize, PathBuf)> = dirs
        .into_iter()
        .map(|d| (d.components().count(), d))
        .collect();
    dirs.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));

    let remove_dir = |(_, d): &(usize, PathBuf)| {
        if args.dry_run {
            if args.verbose {
                println!("would delete {}/", d.display());
            }
            stats.dirs.fetch_add(1, Ordering::Relaxed);
        } else {
            match platform::remove_dir(d) {
                Ok(()) => {
                    if args.verbose {
                        println!("{}/", d.display());
                    }
                    stats.dirs.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    bar.suspend(|| eprintln!("error: rmdir {}: {e}", d.display()));
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if tracking {
            bar.inc(1);
        }
    };

    // Walk the depth-sorted list in contiguous runs of equal depth, removing
    // each run in parallel before descending to the next (shallower) one.
    let mut start = 0;
    while start < dirs.len() {
        let depth = dirs[start].0;
        let end = start + dirs[start..].partition_point(|(d, _)| *d == depth);
        dirs[start..end].par_iter().for_each(remove_dir);
        start = end;
    }
    bar.finish_and_clear();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_root;
    use std::path::Path;

    // Roots must be refused. We test the pure predicate rather than running the
    // binary against a real root — a regression here must surface as a failed
    // assertion, never as an actual attempt to walk a live volume.
    #[test]
    fn detects_roots() {
        #[cfg(unix)]
        {
            assert!(is_root(Path::new("/")));
        }
        // On Windows `std::path::absolute` resolves `.`/`..` lexically (matching
        // GetFullPathName), so disguised roots collapse to a bare root and are
        // caught too. On Unix `absolute` leaves `..` in place by design, so the
        // `..`-disguised form is intentionally NOT caught there — an accepted
        // gap on the non-shipped platform; the bare `/` (the realistic mistake)
        // is what matters and is covered above.
        #[cfg(windows)]
        {
            assert!(is_root(Path::new(r"C:\")));
            assert!(is_root(Path::new(r"C:\.")));
            assert!(is_root(Path::new(r"C:\foo\..")));
        }
    }

    #[test]
    fn allows_non_roots() {
        // A normal nested path is never a root.
        #[cfg(unix)]
        assert!(!is_root(Path::new("/home/user/project/node_modules")));
        #[cfg(windows)]
        assert!(!is_root(Path::new(r"C:\Users\me\project\node_modules")));

        // A relative path qualifies against the cwd (which is not a root in any
        // sane test environment), so it has a parent and is allowed.
        assert!(!is_root(Path::new("node_modules")));
        assert!(!is_root(Path::new(".")));
    }
}
