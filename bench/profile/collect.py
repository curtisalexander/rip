"""Collect one ETW deletion trace, excluding fixture creation and cleanup."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from compare import fixture, measure


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rip", type=Path, required=True)
    parser.add_argument("--perfview", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--shape", choices=["flat", "wide", "dirs", "deep", "readonly"], default="wide")
    parser.add_argument("--entries", type=int, default=20000)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--mode", choices=["cpu", "io"], required=True)
    args = parser.parse_args()
    if os.name != "nt" or args.entries < 1 or args.threads < 1:
        parser.error("Requires Windows and positive entry/worker counts")
    args.output.mkdir(parents=True, exist_ok=True)
    output = args.output.resolve()
    binary = args.rip.resolve(strict=True)
    perfview = args.perfview.resolve(strict=True)
    report = dict(shape=args.shape, mode=args.mode, entries=args.entries, threads=args.threads,
                  binarySha256=hashlib.sha256(binary.read_bytes()).hexdigest())
    with tempfile.TemporaryDirectory(prefix="rip-profile-", ignore_cleanup_errors=True) as work:
        ordinary_root = Path(work).resolve() / "victim"
        root = Path("\\\\?\\" + str(ordinary_root))
        expected = fixture(root, args.shape, args.entries)
        time.sleep(1)
        report["untracedWallMs"] = measure(binary, root, args.threads, expected)
        expected = fixture(root, args.shape, args.entries)
        time.sleep(1)
        log = output / "perfview.log"
        etl = output / "rip.etl"
        events = "Process,Thread,ImageLoad,Profile"
        if args.mode == "io":
            events += ",ContextSwitch,Dispatcher,DiskIO,DiskFileIO,DiskIOInit,FileIO,FileIOInit"
        command = [str(perfview), "/AcceptEula", "/NoGui", "/NoView",
                   f"/LogFile:{log}", f"/DataFile:{etl}", "/Zip:false", "/Merge:true",
                   "/NoRundown", "/MaxCollectSec:120", "/CpuSampleMSec:1",
                   f"/KernelEvents:{events}",
                   "run", str(binary), "--force", "--threads", str(args.threads), str(ordinary_root)]
        result = subprocess.run(command, capture_output=True, timeout=300)
        (output / "collector-output.txt").write_bytes(result.stdout + result.stderr)
        text = log.read_text(encoding="utf-8-sig", errors="replace")
        summary = re.search(r"RIP: deleted (\d+) files, (\d+) dirs .* \((\d+) errors\)", text)
        counts = tuple(map(int, summary.groups())) if summary else None
        if result.returncode or root.exists() or counts != (*expected, 0) or not etl.exists():
            raise RuntimeError(f"Invalid profiled deletion: exit={result.returncode}, counts={counts}; see {log}")
        report.update(expectedFiles=expected[0], expectedDirectories=expected[1], trace=str(etl))
        (output / "capture.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report), flush=True)


if __name__ == "__main__":
    main()
