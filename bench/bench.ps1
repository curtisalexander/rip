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
#>
[CmdletBinding()]
param(
    [int]$Files = 20000,           # total number of files to create
    [int]$Subdirs = 200,           # spread across this many subdirectories
    [int]$Iterations = 3,          # timed runs per tool (median reported)
    [string]$Rip = "rip",          # path to the rip executable (or just "rip" on PATH)
    [switch]$ReadOnly,             # mark files read-only (mimics .git pack files)
    [string]$WorkRoot = $env:TEMP  # where the scratch trees live
)

$ErrorActionPreference = "Stop"

# --- locate rip --------------------------------------------------------------
$ripCmd = Get-Command $Rip -ErrorAction SilentlyContinue
if (-not $ripCmd) {
    throw "Could not find rip executable '$Rip'. Build it (cargo build --release) or pass -Rip <path>."
}
$ripPath = $ripCmd.Source
Write-Host "Using rip: $ripPath" -ForegroundColor Cyan

# --- scratch locations -------------------------------------------------------
$work     = Join-Path $WorkRoot ("rip-bench-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
$template = Join-Path $work "template"
$victim   = Join-Path $work "victim"
$empty    = Join-Path $work "empty"
[System.IO.Directory]::CreateDirectory($work)  | Out-Null
[System.IO.Directory]::CreateDirectory($empty) | Out-Null

# --- build the template tree once (fast, via .NET) ---------------------------
Write-Host ("Generating template: {0:N0} files across {1:N0} dirs..." -f $Files, $Subdirs)
$perDir = [math]::Max(1, [math]::Ceiling($Files / $Subdirs))
for ($d = 0; $d -lt $Subdirs; $d++) {
    $sub = Join-Path $template ("dir_{0:D4}" -f $d)
    [System.IO.Directory]::CreateDirectory($sub) | Out-Null
    for ($f = 0; $f -lt $perDir; $f++) {
        $path = Join-Path $sub ("file_{0:D5}.txt" -f $f)
        [System.IO.File]::WriteAllText($path, "rip benchmark payload")
        if ($ReadOnly) { Set-ItemProperty -LiteralPath $path -Name IsReadOnly -Value $true }
    }
}
$actualFiles = (Get-ChildItem -LiteralPath $template -Recurse -File).Count
Write-Host ("Template ready: {0:N0} files.`n" -f $actualFiles) -ForegroundColor Green

# --- helpers -----------------------------------------------------------------
function New-Victim {
    if (Test-Path -LiteralPath $victim) {
        # Use rip's competitor-agnostic path: robocopy can't delete, so brute it.
        cmd /c "rmdir /s /q `"$victim`"" 2>$null | Out-Null
    }
    # Copy is NOT timed.
    robocopy $template $victim /E /NFL /NDL /NJH /NJS /NP /MT:16 | Out-Null
}

function Measure-Tool {
    param([string]$Name, [scriptblock]$Action)
    $times = @()
    for ($i = 1; $i -le $Iterations; $i++) {
        New-Victim
        [System.GC]::Collect()
        [System.GC]::WaitForPendingFinalizers()
        $ms = (Measure-Command { & $Action }).TotalMilliseconds
        if (Test-Path -LiteralPath $victim) {
            Write-Warning "$Name left files behind on run $i"
        }
        $times += $ms
        Write-Host ("  {0,-12} run {1}: {2,8:N0} ms" -f $Name, $i, $ms)
    }
    $sorted = $times | Sort-Object
    $median = $sorted[[math]::Floor(($sorted.Count - 1) / 2)]
    [pscustomobject]@{ Tool = $Name; MedianMs = [math]::Round($median, 0) }
}

# --- run benchmarks ----------------------------------------------------------
$results = @()

$results += Measure-Tool "rip" {
    # Redirect stderr too: on an interactive console rip would draw a progress
    # bar (stderr is a TTY), and that work would unfairly inflate its timing.
    & $ripPath --force $victim 2>$null | Out-Null
}

$results += Measure-Tool "rmdir" {
    cmd /c "rmdir /s /q `"$victim`"" | Out-Null
}

$results += Measure-Tool "Remove-Item" {
    Remove-Item -LiteralPath $victim -Recurse -Force
}

$results += Measure-Tool "robocopy" {
    robocopy $empty $victim /MIR /NFL /NDL /NJH /NJS /NP /MT:16 | Out-Null
    Remove-Item -LiteralPath $victim -Recurse -Force
}

# --- report ------------------------------------------------------------------
$baseline = ($results | Where-Object Tool -eq "rmdir").MedianMs
Write-Host "`nResults ($actualFiles files, median of $Iterations runs):" -ForegroundColor Cyan
$results |
    Sort-Object MedianMs |
    ForEach-Object {
        $speedup = if ($_.MedianMs -gt 0) { [math]::Round($baseline / $_.MedianMs, 2) } else { "n/a" }
        [pscustomobject]@{
            Tool          = $_.Tool
            "Median (ms)" = $_.MedianMs
            "vs rmdir"    = "${speedup}x"
        }
    } |
    Format-Table -AutoSize

# --- cleanup -----------------------------------------------------------------
cmd /c "rmdir /s /q `"$work`"" 2>$null | Out-Null
Write-Host "Done." -ForegroundColor Green
