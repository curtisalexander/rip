# Windows deletion experiments — September 2026

No experiment established a reliable general speedup. Production deletion code,
Cargo configuration, and the default worker count were restored to the
[original baseline](https://github.com/curtisalexander/rip/commit/68ef64b3b7261d0e136a6cb37ffece44d74d63d5).
The regression tests, paired benchmark, and optional profiling tools were retained.
See the [benchmarking guide](../README.md#comparing-an-optimization-against-the-previous-build)
for current usage. Historical run settings below are not necessarily today's defaults.

## Traversal changes did not yield a general improvement

- [Verbatim-root preparation and depth buckets](https://github.com/curtisalexander/rip/actions/runs/35257861739)
  showed inconsistent gains. Verbatim `PathBuf` joins reparse parents, making
  an already-verbatim walk root a poor shortcut for deep trees.
- [Deleting leaves during enumeration](https://github.com/curtisalexander/rip/actions/runs/35260914751)
  won 9/9 wide-tree pairs at 16 workers, but flat trees at four workers regressed
  from 1,292 ms to 1,603 ms median. An earlier nested-Rayon callback variant
  stalled; the timeout-backed parallel-deletion test guards against recurrence.
- [Ordinary absolute-root preparation](https://github.com/curtisalexander/rip/actions/runs/35262571473)
  won only 5/11 flat, 4/11 wide, 5/11 read-only, 7/11 deep, and 7/11 directory-heavy
  pairs at four workers. A lower median alone was insufficient evidence.

The [source-identical control](https://github.com/curtisalexander/rip/actions/runs/35263855208)
also varied substantially: flat medians were 1,668 versus 1,792 ms, and read-only
medians 1,096 versus 924 ms. This was identical deletion source, not necessarily
byte-identical Windows executables; raw reports preserve both binary hashes.

## Profiling pointed to Windows open/delete/cleanup work

The [six-profile run](https://github.com/curtisalexander/rip/actions/runs/35268061521)
used Windows Server 2022, NTFS, four workers, freshly created fixtures, and optimized
release builds with PDB symbols. All jobs recorded Defender real-time protection
off, verified deletion counts and zero errors, and reported zero lost ETW events.
Fixture creation was outside collection. No security settings were changed;
Defender real-time protection off does not mean filter drivers were unloaded.

CPU-only traces showed these **inclusive CPU shares**, not wall-time shares:

| Workload | Windows delete helper | `CloseHandle` | `CreateFileW` |
|---|---:|---:|---:|
| 20,000 files in one folder | 98.4% | 56.8% | 34.0% |
| 20,000 files across 200 folders | 97.6% | 59.7% | 29.6% |
| 20,032 directories, no files | 52.6% | 30.1% | 17.9% |

Columns overlap: the helper includes the Windows calls. NTFS cleanup dominated
the close path. The jwalk enumeration callback accounted for 37.4% of samples
in the directory-only case, versus 1.4% in the wide-file case.

Detailed I/O traces showed seconds-long cleanup outliers. Their durations overlap
across workers and nested operations; summing them does not give elapsed time.
CPU samples do not measure blocked time. The directory-only CPU job took 38.6 s
untraced and 5.8 s traced on a recreated fixture: these single observations are
diagnostic, not evidence of a speedup or an estimate of tracing overhead.
Detailed I/O collection captures per-operation stacks and adds substantial work;
use the separate CPU-only traces for CPU attribution.

## Parent-relative native opens regressed flat-folder deletion

The [prototype](https://github.com/curtisalexander/rip/commit/f2fd03c)
retained one parent handle per Rayon task and used `NtOpenFile` with `RootDirectory`
and UTF-16 leaf names. It kept POSIX/ignore-readonly disposition, close, four workers,
traversal, and directory deletion unchanged. Cached handles were released before
directory deletion. This was a file-open experiment, not fully handle-relative traversal.

The [first run on C:](https://github.com/curtisalexander/rip/actions/runs/35279004938)
was noisy. Median paired baseline/candidate ratios were 1.007x flat, 1.020x wide,
1.173x deep, 1.184x read-only, and 0.947x directories. Candidate wins were
7/11, 7/11, 6/11, 9/11, and 4/11 respectively.

The [second run on D:](https://github.com/curtisalexander/rip/actions/runs/35279865081)
did not reproduce a benefit:

| Workload | Baseline median | Candidate median | Median paired speedup | Candidate wins |
|---|---:|---:|---:|---:|
| Flat | 497 ms | 737 ms | 0.668x | 0/15 |
| Wide | 373 ms | 370 ms | 0.992x | 6/15 |
| Deep | 365 ms | 373 ms | 0.935x | 6/15 |
| Read-only | 375 ms | 399 ms | 0.924x | 3/15 |
| Directories (unchanged path) | 445 ms | 445 ms | 1.003x | 8/15 |

Ratios above 1 favor the candidate. Median paired ratios differ from ratios of
the two unpaired medians. All 260 timed deletions passed count/error checks;
all ten Windows tests passed on each runner, including prototype tests for
Unicode lengths, parent switching, and junction substitution after caching.
Pre-run checks recorded Defender real-time protection off on every runner.
The regression rejects this prototype, but does not establish its cause or rule
out other handle-relative designs. The prototype and its implementation-specific
tests were reverted; they remain accessible in the linked commit.

## Durable raw results

These copies of selected Actions `comparison.json` artifacts preserve every timed
sample, hash, scratch path, and computed summary. Only CRLF line endings were
normalized to LF for Git:

- [Source-identical control](results/windows-control/): `flat.json` and `readonly.json`
  from run **35263855208**, artifacts `comparison-flat` and `comparison-readonly`.
- [Native-open D: comparison](results/windows-native-open/): all five workloads
  from run **35279865081**, artifacts `comparison-<shape>`. The candidate was
  [this revision](https://github.com/curtisalexander/rip/commit/49f21e5), whose deletion
  source matches the prototype linked above.

Both sets used 10,000 entries, four workers and a one-second settling pause on
hosted Windows Server 2022/NTFS. The control used 11 pairs on C:; the native-open
set used 15 pairs on D:. Baseline was the original revision linked above; the
control candidate was [this revision](https://github.com/curtisalexander/rip/commit/9493e50aed0c39bfbf40ed2fdd5c96e0e83b5254).
These files are historical evidence, not performance thresholds or golden outputs.
Other Actions artifacts can expire; large ETL traces, executables, PDBs, and logs
are intentionally not checked into Git.
