<p align="center">
  <img src="assets/logo/rip-logo.svg" alt="rip logo" width="180" height="180">
</p>

<h1 align="center">rip 🪦</h1>

<p align="center"><strong>R</strong>est <strong>I</strong>n <strong>P</strong>ieces — rip through a directory and delete it <em>as fast as possible</em>.</p>

Born out of frustration with how infernally slow it is to delete `.git`, `.venv`,
`node_modules`, and `target/` on Windows.

## Why it's fast

Windows is slow at deletion because tools delete one file at a time, synchronously,
while Defender scans each one. `rip` instead:

1. **Walks the tree in parallel** (`jwalk`) — multi-threaded enumeration.
2. **Deletes files in parallel** (`rayon`) — saturates the I/O queue.
3. **Deletes read-only files anyway** — read-only `.git` pack files are the
   usual cause of "access denied", so `rip` removes them in the same syscall.
4. **Removes directories deepest-first** so each is empty when reached.

It also addresses every entry through a `\\?\` verbatim path, so deeply nested
trees that exceed Windows' legacy 260-character `MAX_PATH` limit — the usual
`node_modules` situation — delete without error.

## Usage

```
rip <path>...            # delete one or more paths (warns, then asks to confirm)
rip -f .git .venv        # --force: skip all warnings + confirmation, just rip
rip -t node_modules      # --trash: move to Recycle Bin instead (recoverable)
rip -n target            # --dry-run: show what would be deleted, delete nothing
rip -j 16 node_modules   # use 16 worker threads
rip -v .git              # print every deleted path
```

By default `rip` deletes **permanently** (no Recycle Bin), so it shows a loud
warning and asks you to confirm. The warning spells out exactly what "rip" does:

- **No Recycle Bin** — deletion is irreversible.
- **Ignores read-only** (`FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE`) —
  read-only files like `.git` packs are deleted anyway.
- **POSIX semantics** (`FILE_DISPOSITION_FLAG_POSIX_SEMANTICS`) — files still in
  use are force-removed.

Use `-f`/`--force` (alias `-y`/`--yes`) to skip the warning and confirmation, or
`-t`/`--trash` to move to the Recycle Bin / Trash instead (recoverable, safer).

Large deletes show a scan spinner and a progress bar on an interactive terminal.
They stay out of the way otherwise — suppressed when output is piped or
redirected, under `--verbose` (which prints every path), and under `--dry-run` —
so scripted and benchmarked runs pay nothing for them.

## Speed vs. safety

`rip` exists to delete throwaway build and dependency trees (`.git`, `.venv`,
`node_modules`, `target/`) as fast as the disk allows. That speed comes from
doing less ceremony than Explorer and the built-ins — the right trade for
scratch directories, the wrong one for files you might want back. Here's exactly
what is and isn't traded.

**What `rip` does that the Windows built-ins don't** — this is where the speed
comes from:

- **Deletes in parallel** instead of one file at a time, saturating the disk
  queue rather than waiting on each delete round-trip.
- **POSIX-semantics deletion** — the directory entry vanishes immediately rather
  than lingering until the last handle closes, so the tree is gone *now*.
- **Ignores the read-only attribute** in the same syscall — no clear-then-retry
  dance on `.git` pack files.
- **Force-removes files still in use** instead of erroring out.
- **Handles `\\?\` long paths** that trip other tools on deep `node_modules`.

**What you give up for that speed**, versus deleting in Explorer:

- **It's permanent.** No Recycle Bin by default, so a mistake is unrecoverable.
  Pass `-t`/`--trash` when you want the safety net (it's slower).
- **Fewer guardrails.** Read-only and in-use files that Explorer would stop and
  ask about are removed without a per-file prompt — `rip` asks once, up front,
  for the whole operation instead.

**What `rip` does _not_ trade away:**

- **It never follows a symlink or junction out of the tree.** A reparse point is
  deleted as a *link*; the data it points at is never touched. This is the one
  guarantee a fast recursive deleter must never break, and an end-to-end test
  (`tests/reparse_safety.rs`) holds it to that.
- **It refuses to rip a filesystem root.** A bare `rip C:\` (or `/`, or
  `\\server\share`) is almost always an accident — a stray argument, an
  unexpanded variable, a misplaced slash. `rip` rejects whole-volume roots
  outright, *before* any confirmation and even under `-f`/`--force`. It's a tool
  for throwaway subtrees, not for erasing a drive.
- **It still confirms by default.** Unlike `rmdir /s /q`, a bare `rip <path>`
  prints a loud warning and waits for you to type `y`.

**What Windows still does that `rip` doesn't:**

- **Recycle Bin by default** — `rip` is permanent unless you pass `--trash`.
- **Defender scans every file as it's deleted.** `rip` doesn't disable that; it
  just parallelizes the deletes so the scanning overlaps instead of serializing.
  There is deliberately **no** Defender-exclusion feature — `rip` never weakens
  your security posture to go faster.

### Soft links vs. hard links

Windows has two unrelated kinds of "link," and `rip` treats them differently
because they *are* different things:

- **Soft links** are *pointers* to another path — a separate filesystem object
  (a "reparse point") whose whole job is to redirect to a target somewhere else.
  This covers **symbolic links** (`mklink` for files, `mklink /D` for
  directories) and **junctions** (`mklink /J`, the elevation-free directory
  redirect). The target can live anywhere — another folder, another volume, your
  home directory.
- **Hard links** are *not* pointers. A hard link is just an additional name for
  one and the same file data on one volume (`mklink /H` / `CreateHardLink`). The
  "original" and the hard link are co-equal; neither is more real than the other,
  and the bytes on disk are reference-counted, freed only when the **last** name
  is removed. Hard links exist for files only, never directories, and never
  across volumes.

How `rip` behaves with each:

| Kind | What `rip` deletes | What survives |
| --- | --- | --- |
| **Soft link** (symlink / junction) | The link itself — always. `rip` opens every entry with `FILE_FLAG_OPEN_REPARSE_POINT` and walks with `follow_links(false)`, so it removes the reparse point as a leaf and **never traverses into the target**. | The target directory/file and everything under it, completely untouched — even when the link sits *inside* the tree you're ripping. |
| **Hard link** | The one name you pointed it at (that directory entry). There's no reparse point and nothing to "follow" — a hard link *is* the file. | The file's data, as long as **another** hard link to it still exists elsewhere. Only deleting the final remaining link frees the bytes. |

The soft-link guarantee is the dangerous one — a junction in `.venv` pointing at
your home directory must never let a recursive deleter escape the tree — so it's
the one held to an end-to-end test (`tests/reparse_safety.rs`), which rips a tree
containing a junction/symlink and verifies the link dies while the target lives.

The hard-link behavior needs no special handling: because a hard link is an
ordinary directory entry, deleting it is an ordinary delete. You only lose data
if you remove the *last* link to it — exactly as any other delete on Windows
would behave.

### Dialing the safety back up

The aggressive defaults are opt-out — each trade has a flag that hands the
guardrail back:

| Want it back | Flag | Effect |
| --- | --- | --- |
| Recoverability (Recycle Bin) | `-t` / `--trash` | Moves paths to the Recycle Bin / Trash instead of deleting — fully recoverable, at the cost of speed. |
| A look before you leap | `-n` / `--dry-run` | Prints exactly what *would* be deleted and touches nothing. |
| Per-run confirmation | *(default)* | A bare `rip <path>` already warns and waits for a `y`; only `-f` / `--force` removes that prompt. |

So the fast, permanent behavior is what you get by default, but
`rip -t <path>` is a recoverable delete and `rip -n <path>` is a safe preview —
reach for them whenever you'd rather trade speed for a safety net.

### Known limitations

- **Not atomic (a TOCTOU window).** `rip` works in two passes — it walks the
  tree to enumerate every entry, then deletes. It does not lock the tree in
  between, so if something *adds* files under the target while a delete is in
  flight, those new entries may be removed (they're under a path you asked to
  delete) or may cause a parent directory to fail to remove (it's no longer
  empty). This is inherent to any fast, parallel, non-transactional deleter.
  Don't point `rip` at a tree another process is actively writing into; for a
  build/dependency directory you've stopped using — its intended use — this
  window is irrelevant.
- **Root guard is exact, not heuristic.** The refusal covers filesystem, drive,
  and UNC *roots* (`/`, `C:\`, `\\server\share`). It does **not** second-guess a
  non-root path like `C:\Users\you` — `rip` deletes exactly what you name. The
  `-n`/`--dry-run` preview is there for when you want to look before you leap.

## Installation

`rip` is a Rust binary wrapped in a Python wheel (via [maturin](https://www.maturin.rs/)),
so it installs like any Python tool — **no Rust toolchain required** on the target machine.

### With `uv tool install` (recommended)

```
uv tool install \
  --no-index \
  --find-links https://github.com/curtisalexander/rip/releases/expanded_assets/v0.1.2 \
  rip
```

The `rip` command is then available on your `PATH`.

## Building

### Rust binary directly (dev loop)

```
cargo build --release        # -> target/release/rip
```

### Wheel for the current platform

```
pip install maturin
maturin build --release      # -> target/wheels/rip-*.whl
```

### Windows (MSVC)

`rip` targets `x86_64-pc-windows-msvc`. The reliable way to produce the Windows
wheel is GitHub CI: `.github/workflows/ci.yml` builds it on a `windows-latest`
runner (native MSVC, no mingw) and attaches the wheel to a GitHub Release on
every push to `main`.

To build locally on a Windows box:

```
maturin build --release --target x86_64-pc-windows-msvc
```

(Cross-compiling MSVC from macOS/Linux is possible with `cargo-xwin` but is not
the supported path — let CI do it.)

> **Windows is the only supported target.** The code stays cross-platform so it
> can be built and tested on a dev machine (e.g. macOS), but CI, releases, and
> the shipped wheel are Windows-only (`x86_64-pc-windows-msvc`).

## Benchmarking

`bench/bench.ps1` compares `rip` against the Windows built-ins on a synthetic
tree. Each tool deletes a fresh copy; the copy isn't timed.

```powershell
cargo build --release
pwsh ./bench/bench.ps1 -Rip .\target\release\rip.exe -Files 50000 -Subdirs 500 -Iterations 5
# add -ReadOnly to mimic .git pack files (read-only attribute set)
# add -NoEmoji for ASCII tags instead of emoji on the legacy console (conhost)
```

Contenders: `rip --force`, `cmd rmdir /s /q`, PowerShell `Remove-Item -Recurse
-Force`, and the `robocopy /MIR` empty-mirror trick. Reports the median per tool
and the speedup vs. `rmdir`. Output is emoji-styled with per-phase timing; pass
`-NoEmoji` if your terminal renders emoji as boxes.

## Testing

```
cargo test                  # runs the safety + long-path tests
```

`tests/reparse_safety.rs` verifies the most dangerous failure mode: ripping a
tree containing a symlink/junction must delete the **link**, never traverse it
and destroy the data it points at. It runs on Unix (symlink) and on Windows
(junction via `mklink /J`, which needs no elevation), so it executes in CI. A
third case covers a Windows *directory symlink* (`mklink /D`) — a distinct
reparse kind from a junction — and is skipped, not failed, when the environment
can't create one (it needs Developer Mode or elevation).

`tests/long_path.rs` rips a tree whose paths exceed the legacy 260-character
`MAX_PATH`, exercising the `\\?\` verbatim-path handling on Windows.

## Roadmap

- [x] Windows `FILE_DISPOSITION_POSIX_SEMANTICS` for true immediate deletion.
- [x] `--trash` mode (Recycle Bin / Trash, recoverable).
- [x] Loud warnings + confirmation, with `--force` to skip.
- [x] Reparse-point safety test (never traverse a symlink/junction), covering
      junctions and directory symlinks.
- [x] Root guard — refuse to rip a filesystem/drive/UNC root, even under `--force`.
- [x] Benchmark harness vs. `rmdir /s /q`, `Remove-Item`, and `robocopy /MIR`.
- [x] Long-path support (`\\?\`) for trees past the 260-char `MAX_PATH`.
- [x] Progress bar for very large deletes.

## License

[MIT](LICENSE) © Curtis Alexander
