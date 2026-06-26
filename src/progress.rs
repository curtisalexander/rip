//! Optional progress UI for large deletes (indicatif).
//!
//! Bars are drawn only when they actually help: an interactive stderr, not
//! `--verbose` (which prints every path) and not `--dry-run`, and the delete
//! bar only once a tree is big enough to be worth watching. When a bar would
//! not be shown it is a hidden no-op, so call sites stay branch-free — and the
//! hot delete loop checks a captured `bool` (`!bar.is_hidden()`) so a disabled
//! bar costs nothing per file, keeping piped/CI/benchmark runs at full speed.

use std::io::IsTerminal;

use indicatif::{ProgressBar, ProgressStyle};

/// Below this many entries a delete finishes faster than a bar could
/// meaningfully render, so we don't bother drawing one.
const MIN_ENTRIES_FOR_BAR: u64 = 2_000;

/// The scan spinner is refreshed (and first drawn) every this many entries.
/// Because the count is the only thing that drives a redraw, a tree smaller
/// than this never draws a spinner at all — no flash on quick deletes — and the
/// threshold is kept near `MIN_ENTRIES_FOR_BAR` so the scan spinner and the
/// delete bar appear together, only for trees big enough to warrant either.
pub const SCAN_REFRESH: usize = 2_048;

/// Whether progress bars should be drawn at all for this run.
pub fn enabled(verbose: bool, dry_run: bool) -> bool {
    !verbose && !dry_run && std::io::stderr().is_terminal()
}

/// A spinner that counts entries during the tree walk. It is driven entirely by
/// the caller's periodic `set_position` (see `SCAN_REFRESH`) rather than a
/// steady tick, so it stays silent until a scan is genuinely large.
pub fn scanner(show: bool) -> ProgressBar {
    if !show {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner:.cyan} scanning… {human_pos} entries").unwrap(),
    );
    pb
}

/// A determinate bar for the deletion phase, sized to `total` (files + dirs).
/// Hidden for small trees so it never flashes on a quick delete.
pub fn deleter(show: bool, total: u64) -> ProgressBar {
    if !show || total < MIN_ENTRIES_FOR_BAR {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.red} ripping [{bar:32.red/dim}] {human_pos}/{human_len} ({eta})",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb
}
