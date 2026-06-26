//! Attractive command-line warnings and confirmation prompts.

use std::io::{self, Write};
use std::path::PathBuf;

use colored::Colorize;

/// What kind of destruction we're about to do — drives the warning styling.
pub enum Mode {
    /// Permanent, irreversible deletion via the raw OS API.
    Rip,
    /// Recoverable move to the Recycle Bin / Trash.
    Trash,
}

const RULE_WIDTH: usize = 66;

/// Show a mode-appropriate warning banner and ask the user to confirm.
/// Returns `true` if the user agreed to proceed.
pub fn confirm(paths: &[PathBuf], mode: Mode) -> bool {
    match mode {
        Mode::Rip => warn_rip(paths),
        Mode::Trash => warn_trash(paths),
    }
}

fn warn_rip(paths: &[PathBuf]) -> bool {
    let rule = "━".repeat(RULE_WIDTH);
    eprintln!();
    eprintln!("{}", rule.bright_red());
    eprintln!(
        "{}",
        "  ⚠  RIP  —  PERMANENT DELETION  ⚠".bright_red().bold()
    );
    eprintln!("{}", rule.bright_red());
    eprintln!();
    eprintln!(
        "  About to {} delete {} path(s):",
        "PERMANENTLY".bright_red().bold(),
        paths.len().to_string().bold()
    );
    list_paths(paths, "•".bright_red());
    eprintln!();
    eprintln!("  This is what \"rip\" means:");
    eprintln!(
        "    {}  No Recycle Bin — {}.",
        "✗".bright_red(),
        "this cannot be undone".bright_white().bold()
    );
    eprintln!(
        "    {}  Read-only files are deleted anyway (IGNORE_READONLY).",
        "✗".bright_red()
    );
    eprintln!(
        "    {}  Files still in use are force-removed (POSIX_SEMANTICS).",
        "✗".bright_red()
    );
    eprintln!();
    prompt(&format!(
        "Type {} to rip it out — anything else aborts",
        "y".bright_red().bold()
    ))
}

fn warn_trash(paths: &[PathBuf]) -> bool {
    let rule = "━".repeat(RULE_WIDTH);
    eprintln!();
    eprintln!("{}", rule.yellow());
    eprintln!("{}", "  ♻  RIP  —  MOVE TO RECYCLE BIN".yellow().bold());
    eprintln!("{}", rule.yellow());
    eprintln!();
    eprintln!(
        "  About to move {} path(s) to the Recycle Bin ({}):",
        paths.len().to_string().bold(),
        "recoverable".green()
    );
    list_paths(paths, "•".yellow());
    eprintln!();
    prompt(&format!(
        "Type {} to continue — anything else aborts",
        "y".yellow().bold()
    ))
}

fn list_paths(paths: &[PathBuf], bullet: colored::ColoredString) {
    const MAX_SHOWN: usize = 12;
    for p in paths.iter().take(MAX_SHOWN) {
        eprintln!("    {} {}", bullet, p.display().to_string().bold());
    }
    if paths.len() > MAX_SHOWN {
        eprintln!("    {} … and {} more", bullet, paths.len() - MAX_SHOWN);
    }
}

fn prompt(message: &str) -> bool {
    eprint!("  {} {} ", message, "▸".bright_cyan().bold());
    io::stderr().flush().ok();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "rip"
    )
}
