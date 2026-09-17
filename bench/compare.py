"""Paired end-to-end rip benchmark; standard library only, Windows or Unix.

Fixtures are freshly generated outside timing. Both binaries run on the same
filesystem, with captured output (no progress UI). Never accepts failed deletes.
This measures warm, freshly created trees, not cold-cache or secure erasure speed.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import re
import stat
import statistics
import subprocess
import tempfile
import time


def fixture(root, shape, entries):
    root.mkdir()
    dirs = {root}
    for i in range(entries):
        if shape == "flat":
            parent = root
        elif shape == "dirs":
            parent = root / f"group-{i % 31}" / f"empty-{i}"
        elif shape == "deep":
            parent = root / f"group-{i % 31}"
            for depth in range(i % 12 + 1):
                parent /= f"level-{depth}-abcdefghijklmnop"
        else:
            parent = root / f"package-{i % 200}"
        parent.mkdir(parents=True, exist_ok=True)
        ancestor = parent
        while ancestor not in dirs:
            dirs.add(ancestor)
            ancestor = ancestor.parent
        if shape != "dirs":
            file = parent / f"file-{i}.txt"
            file.write_bytes(b"rip benchmark payload\n")
            if shape == "readonly":
                file.chmod(stat.S_IREAD)
    return (0 if shape == "dirs" else entries), len(dirs)


def measure(binary, root, threads, expected):
    command = [str(binary), "--force"]
    if threads:
        command += ["--threads", str(threads)]
    # Pass the ordinary absolute path a user would supply, not Python's
    # verbatim fixture path: path preparation is part of the measured work.
    target = str(root)
    if os.name == "nt":
        if target.startswith("\\\\?\\UNC\\"):
            target = "\\\\" + target[8:]
        elif target.startswith("\\\\?\\"):
            target = target[4:]
    command.append(target)
    start = time.perf_counter_ns()
    result = subprocess.run(command, capture_output=True, timeout=300)
    elapsed = (time.perf_counter_ns() - start) / 1_000_000
    summary = re.search(
        rb"RIP: deleted (\d+) files, (\d+) dirs .* \((\d+) errors\)", result.stderr
    )
    counts = tuple(map(int, summary.groups())) if summary else None
    if result.returncode or root.exists() or counts != (*expected, 0):
        raise RuntimeError(f"Invalid deletion: {command}: {result.stderr!r}")
    return elapsed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--shapes", nargs="+", default=["flat", "wide", "dirs", "deep", "readonly"],
                        choices=["flat", "wide", "dirs", "deep", "readonly"])
    parser.add_argument("--entries", type=int, default=10000)
    parser.add_argument("--iterations", type=int, default=7)
    parser.add_argument("--threads", nargs="+", type=int, default=[0, 4, 16, 32],
                        help="0 uses rip's default logical CPU count")
    parser.add_argument("--work-root", type=Path, help="scratch parent on the filesystem to measure")
    parser.add_argument("--settle-seconds", type=float, default=0,
                        help="untimed pause after fixture creation to reduce background-write overlap")
    parser.add_argument("--output", type=Path, default=Path("comparison.json"))
    args = parser.parse_args()
    if (args.entries < 1 or args.iterations < 1 or any(t < 0 for t in args.threads)
            or not 0 <= args.settle_seconds < float("inf")):
        parser.error("entries/iterations must be positive; threads and finite settle time must be nonnegative")
    binaries = {"baseline": args.baseline.resolve(strict=True),
                "candidate": args.candidate.resolve(strict=True)}
    report = {
        "platform": platform.platform(), "cpu_count": os.cpu_count(),
        "entries": args.entries, "iterations": args.iterations,
        "settle_seconds": args.settle_seconds,
        "rayon_num_threads": os.environ.get("RAYON_NUM_THREADS"),
        "sha256": {name: hashlib.sha256(path.read_bytes()).hexdigest()
                   for name, path in binaries.items()},
        "samples": [], "summary": [],
    }
    rng = random.Random(2026)
    with tempfile.TemporaryDirectory(prefix="rip-compare-", dir=args.work_root,
                                     ignore_cleanup_errors=True) as work:
        # Explicit verbatim paths also let Python create deep Windows fixtures.
        work = os.path.abspath(work)
        if os.name == "nt" and not work.startswith("\\\\?\\"):
            work = "\\\\?\\UNC\\" + work[2:] if work.startswith("\\\\") else "\\\\?\\" + work
        root = Path(work) / "victim"
        report["scratch_path"] = work
        cases = [(shape, threads) for shape in args.shapes for threads in args.threads]
        rng.shuffle(cases)
        for shape, threads in cases:
            times = {name: [] for name in binaries}
            first_order = list(binaries)
            rng.shuffle(first_order)
            # One untimed warmup per binary/configuration, then paired samples.
            for iteration in range(-1, args.iterations):
                order = first_order if iteration % 2 == 0 else first_order[::-1]
                for name in order:
                    expected = fixture(root, shape, args.entries)
                    if args.settle_seconds:
                        time.sleep(args.settle_seconds)
                    ms = measure(binaries[name], root, threads, expected)
                    if iteration >= 0:
                        times[name].append(ms)
                        report["samples"].append(dict(shape=shape, threads=threads,
                                                      iteration=iteration, binary=name, ms=ms))
            ratios = [a / b for a, b in zip(times["baseline"], times["candidate"])]
            row = dict(shape=shape, threads=threads,
                       baseline_ms=statistics.median(times["baseline"]),
                       candidate_ms=statistics.median(times["candidate"]),
                       paired_speedup=statistics.median(ratios),
                       candidate_wins=sum(r > 1 for r in ratios),
                       min_speedup=min(ratios), max_speedup=max(ratios))
            report["summary"].append(row)
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            print(f"{shape:8} j={threads:2} baseline={row['baseline_ms']:.1f} ms "
                  f"candidate={row['candidate_ms']:.1f} ms "
                  f"paired={row['paired_speedup']:.3f}x "
                  f"wins={row['candidate_wins']}/{args.iterations}", flush=True)


if __name__ == "__main__":
    main()
