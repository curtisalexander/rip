<#
.SYNOPSIS
    Benchmark `rip` against Windows' built-in directory deleters.

.DESCRIPTION
    Generates a synthetic directory tree once, then repeatedly: copies a fresh
    victim from the template (NOT timed) and times how long each tool takes to
    delete it. Reports the median per tool and the speedup vs. `rmdir /s /q`.

    Contenders:
      * rip          - this project (rip --force)
      * rmdir        - cmd.exe `rmdir /s /q`
      * Remove-Item  - PowerShell `Remove-Item -Recurse -Force`
      * robocopy     - the classic `robocopy /MIR` empty-mirror delete trick

.EXAMPLE
    pwsh ./bench/bench.ps1 -Files 50000 -Subdirs 500 -Iterations 5

.EXAMPLE
    # Point at a release build you just produced:
    pwsh ./bench/bench.ps1 -Rip ..\target\release\rip.exe -ReadOnly

.EXAMPLE
    # Legacy console (conhost) that can't render emoji: use ASCII tags instead.
    pwsh ./bench/bench.ps1 -NoEmoji
#>
[CmdletBinding()]
param(
    [int]$Files = 20000,           # total number of files to create
    [int]$Subdirs = 200,           # spread across this many subdirectories
    [int]$Iterations = 3,          # timed runs per tool (median reported)
    [string]$Rip = "rip",          # path to the rip executable (or just "rip" on PATH)
    [switch]$ReadOnly,             # mark files read-only (mimics .git pack files)
    [string]$WorkRoot = $env:TEMP, # where the scratch trees live
    [switch]$NoEmoji               # use ASCII tags instead of emoji (legacy conhost)
)

$ErrorActionPreference = "Stop"

# --- logging helpers ---------------------------------------------------------
# Emoji + box-drawing need UTF-8; legacy conhost defaults to a codepage that
# renders them as mojibake otherwise. Best-effort: ignore if the host refuses.
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

# Icon set: emoji by default, ASCII tags under -NoEmoji (legacy conhost). Keyed
# by meaning so call sites read the same regardless of which set is active.
$script:Ico = if ($NoEmoji) {
    @{ rocket="::"; gen="[gen]"; ok="[ok]"; flag="[go]"; tool="*"; run="[>]";
       copy="[cp]"; gc="[gc]"; timing="[t+]"; result="[t=]"; median="[med]";
       broom="[rm]"; trophy=">>"; medal="[win]"; sparkle="[done]" }
} else {
    @{ rocket="🚀"; gen="📁"; ok="✅"; flag="🏁"; tool="🛠️"; run="▶️";
       copy="📋"; gc="⚙️"; timing="⏱️"; result="⏹️"; median="📊";
       broom="🧹"; trophy="🏆"; medal="🥇"; sparkle="✨" }
}

$script:bench_start = Get-Date
$script:rule_width  = 52

# A timestamped line, optionally prefixed with an emoji icon.
function Log {
    param([string]$Message, [ConsoleColor]$Color = [ConsoleColor]::Gray, [string]$Icon = "")
    $elapsed = (New-TimeSpan -Start $script:bench_start).TotalSeconds
    $prefix  = if ($Icon) { "$Icon " } else { "" }
    Write-Host ("[{0,6:N1}s] {1}{2}" -f $elapsed, $prefix, $Message) -ForegroundColor $Color
}

# A full-width horizontal rule.
function Rule {
    param([ConsoleColor]$Color = [ConsoleColor]::DarkGray, [char]$Char = '─')
    Write-Host ([string]$Char * $script:rule_width) -ForegroundColor $Color
}

# A bold header: heavy rule, title line(s), heavy rule. Emoji-safe (no right
# border to misalign against double-width glyphs).
function Banner {
    param([string[]]$Lines, [ConsoleColor]$Color = [ConsoleColor]::Cyan)
    Write-Host ""
    Rule $Color '═'
    foreach ($l in $Lines) { Write-Host ("  " + $l) -ForegroundColor $Color }
    Rule $Color '═'
}

# A lighter section divider, e.g. "──── 🛠️  Tool: rip ────".
function Section {
    param([string]$Title, [ConsoleColor]$Color = [ConsoleColor]::Cyan)
    Write-Host ""
    Write-Host ("──── {0} " -f $Title).PadRight($script:rule_width, '─') -ForegroundColor $Color
}

# Run a scriptblock, print what it's doing, and report how long it took.
function Step {
    param([string]$Message, [scriptblock]$Action, [ConsoleColor]$Color = [ConsoleColor]::DarkGray, [string]$Icon = "")
    Log $Message $Color $Icon
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Action
    $sw.Stop()
    Log ("  └─ done in {0:N0} ms" -f $sw.Elapsed.TotalMilliseconds) $Color
}

# --- locate rip --------------------------------------------------------------
$ripCmd = Get-Command $Rip -ErrorAction SilentlyContinue
if (-not $ripCmd) {
    throw "Could not find rip executable '$Rip'. Build it (cargo build --release) or pass -Rip <path>."
}
$ripPath = $ripCmd.Source
Banner @(
    "$($script:Ico.rocket)  rip benchmark",
    ("{0:N0} files × {1:N0} dirs · {2} timed run(s) per tool{3}" -f $Files, $Subdirs, $Iterations, $(if ($ReadOnly) { " · read-only" } else { "" })),
    "rip: $ripPath"
) Cyan

# --- scratch locations -------------------------------------------------------
$work     = Join-Path $WorkRoot ("rip-bench-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
$template = Join-Path $work "template"
$victim   = Join-Path $work "victim"
$empty    = Join-Path $work "empty"
[System.IO.Directory]::CreateDirectory($work)  | Out-Null
[System.IO.Directory]::CreateDirectory($empty) | Out-Null

# --- build the template tree once (fast, via .NET) ---------------------------
$perDir = [math]::Max(1, [math]::Ceiling($Files / $Subdirs))
Log ("Generating template tree (this happens ONCE): {0:N0} files x {1:N0} dirs (~{2:N0}/dir){3} ..." -f `
        $Files, $Subdirs, $perDir, $(if ($ReadOnly) { ", read-only" } else { "" })) Yellow $script:Ico.gen
$genSw = [System.Diagnostics.Stopwatch]::StartNew()
for ($d = 0; $d -lt $Subdirs; $d++) {
    $sub = Join-Path $template ("dir_{0:D4}" -f $d)
    [System.IO.Directory]::CreateDirectory($sub) | Out-Null
    for ($f = 0; $f -lt $perDir; $f++) {
        $path = Join-Path $sub ("file_{0:D5}.txt" -f $f)
        [System.IO.File]::WriteAllText($path, "rip benchmark payload")
        if ($ReadOnly) { Set-ItemProperty -LiteralPath $path -Name IsReadOnly -Value $true }
    }
    # Progress every ~10% so a slow generate doesn't look like a hang.
    if ($Subdirs -ge 10 -and ($d + 1) % [math]::Ceiling($Subdirs / 10) -eq 0) {
        Log ("  generated {0:N0}/{1:N0} dirs..." -f ($d + 1), $Subdirs)
    }
}
$genSw.Stop()
$actualFiles = (Get-ChildItem -LiteralPath $template -Recurse -File).Count
Log ("Template ready: {0:N0} files in {1:N1}s. Template lives at: {2}" -f `
        $actualFiles, $genSw.Elapsed.TotalSeconds, $template) Green $script:Ico.ok

# --- helpers -----------------------------------------------------------------
function New-Victim {
    if (Test-Path -LiteralPath $victim) {
        # Use a competitor-agnostic path: brute-delete the leftover victim first.
        Step "  setup: deleting previous victim (rmdir /s /q) [NOT timed]" {
            cmd /c "rmdir /s /q `"$victim`"" 2>$null | Out-Null
        } DarkGray $script:Ico.broom
    }
    # Copy is NOT part of any tool's measured time -- but it's the main reason
    # the overall benchmark feels slow, so we announce + time it explicitly.
    Step ("  setup: copying fresh victim from template (robocopy /E /MT:16) [NOT timed]" -f $victim) {
        robocopy $template $victim /E /NFL /NDL /NJH /NJS /NP /MT:16 | Out-Null
    } DarkGray $script:Ico.copy
}

function Measure-Tool {
    param([string]$Name, [string]$CommandText, [scriptblock]$Action)
    Section ("{0}  Tool: {1}  ({2} timed run(s))" -f $script:Ico.tool, $Name, $Iterations) Cyan
    Log ("command: {0}" -f $CommandText) DarkCyan
    $times = @()
    for ($i = 1; $i -le $Iterations; $i++) {
        Log ("run {0}/{1}" -f $i, $Iterations) White $script:Ico.run
        New-Victim
        Step "  setup: forcing GC before timing [NOT timed]" {
            [System.GC]::Collect()
            [System.GC]::WaitForPendingFinalizers()
        } DarkGray $script:Ico.gc
        Log ("  TIMING the delete now ({0}) ..." -f $Name) Magenta $script:Ico.timing
        $ms = (Measure-Command { & $Action }).TotalMilliseconds
        if (Test-Path -LiteralPath $victim) {
            Write-Warning "$Name left files behind on run $i"
        }
        $times += $ms
        Log ("  {0} run {1}: {2,8:N0} ms (TIMED delete)" -f $Name, $i, $ms) Magenta $script:Ico.result
    }
    $sorted = $times | Sort-Object
    $median = $sorted[[math]::Floor(($sorted.Count - 1) / 2)]
    Log ("{0} median: {1:N0} ms" -f $Name, $median) Green $script:Ico.median
    [pscustomobject]@{ Tool = $Name; MedianMs = [math]::Round($median, 0) }
}

# --- run benchmarks ----------------------------------------------------------
Log ("Starting timed comparisons. Each tool does {0} copy+delete cycle(s); only the delete is timed." -f $Iterations) Yellow $script:Ico.flag
$results = @()

$results += Measure-Tool "rip" "$ripPath --force <victim>" {
    # Redirect stderr too: on an interactive console rip would draw a progress
    # bar (stderr is a TTY), and that work would unfairly inflate its timing.
    & $ripPath --force $victim 2>$null | Out-Null
}

$results += Measure-Tool "rmdir" "cmd /c rmdir /s /q <victim>" {
    cmd /c "rmdir /s /q `"$victim`"" | Out-Null
}

$results += Measure-Tool "Remove-Item" "Remove-Item -Recurse -Force <victim>" {
    Remove-Item -LiteralPath $victim -Recurse -Force
}

$results += Measure-Tool "robocopy" "robocopy <empty> <victim> /MIR  then  Remove-Item <victim>" {
    robocopy $empty $victim /MIR /NFL /NDL /NJH /NJS /NP /MT:16 | Out-Null
    Remove-Item -LiteralPath $victim -Recurse -Force
}

# --- report ------------------------------------------------------------------
$baseline = ($results | Where-Object Tool -eq "rmdir").MedianMs
$ranked   = $results | Sort-Object MedianMs
$fastest  = $ranked[0]
$slowest  = ($ranked | Select-Object -Last 1).MedianMs

Banner @(
    "$($script:Ico.trophy)  Results — median TIMED delete only",
    ("{0:N0} files · median of {1} run(s) · copy/setup time excluded" -f $actualFiles, $Iterations)
) Cyan

# Column widths so everything lines up regardless of tool-name length.
$nameW = ($ranked | ForEach-Object { $_.Tool.Length } | Measure-Object -Maximum).Maximum
$barMax = 18   # characters for the longest (slowest) bar

foreach ($r in $ranked) {
    $isWinner = $r.Tool -eq $fastest.Tool
    $medal    = if ($isWinner) { $script:Ico.trophy } else { "  " }
    $speedup  = if ($r.MedianMs -gt 0) { $baseline / $r.MedianMs } else { 0 }

    # Bar length is proportional to time: slowest = full bar, fastest = short.
    $frac     = if ($slowest -gt 0) { $r.MedianMs / $slowest } else { 0 }
    $full     = [math]::Floor($frac * $barMax)
    $bar      = ("█" * $full).PadRight($barMax, '░')

    $color    = if ($isWinner) { [ConsoleColor]::Green }
                elseif ($r.MedianMs -le $baseline) { [ConsoleColor]::Gray }
                else { [ConsoleColor]::DarkGray }

    Write-Host ("  {0} {1}  {2,9:N0} ms  {3}  {4,5:N2}x" -f `
        $medal, $r.Tool.PadRight($nameW), $r.MedianMs, $bar, $speedup) -ForegroundColor $color
}
Write-Host ""
Log ("Winner: {0} — {1:N0} ms ({2:N2}x faster than rmdir baseline)" -f `
        $fastest.Tool, $fastest.MedianMs, ($baseline / $fastest.MedianMs)) Green $script:Ico.medal

# --- cleanup -----------------------------------------------------------------
Step ("Cleaning up scratch tree: {0}" -f $work) {
    cmd /c "rmdir /s /q `"$work`"" 2>$null | Out-Null
} DarkGray $script:Ico.broom
Banner @(
    ("$($script:Ico.sparkle)  Done — total wall-clock {0:N1}s" -f (New-TimeSpan -Start $script:bench_start).TotalSeconds)
) Green
