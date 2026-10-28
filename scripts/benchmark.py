"""Reproducible warm-cache CLI benchmark; Python 3.10+, no pip packages.

All tools search the same ASCII corpus, emit matching lines to a null sink,
and have their result counts checked before timing. Exit codes 0/1 are valid.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess
import time

PATTERNS = {
    "missing": "QF_MISSING_58eb",
    "sparse": "QF_RARE_9c8d",
    "dense": "QF_DENSE_e27a",
}


def corpus(root, config):
    marker = root / ".qfind-benchmark.json"
    if marker.exists():
        if json.loads(marker.read_text()) != config:
            raise SystemExit("Dataset settings changed; choose a different --data directory.")
        expected = config["files"] + config["large_files"]
        if len(list(root.rglob("*.txt"))) != expected:
            raise SystemExit("Dataset is incomplete; choose a new --data directory.")
        return
    if root.exists() and any(root.iterdir()):
        raise SystemExit("Refusing to overwrite a nonempty dataset directory.")
    root.mkdir(parents=True, exist_ok=True)
    line = b"let account_id = 1357; const status = 'ready'; // normal searchable source line\r\n"
    dense = b"let record_id = 2468; // QF_DENSE_e27a\r\n"
    block = line * 63 + dense
    header = b"// QF_RARE_9c8d\r\n"

    def write_file(path, size):
        path.parent.mkdir(parents=True, exist_ok=True)
        body = header + (block * (size // len(block) + 1))[:size - len(header)]
        # Keep a newline at EOF, so counting output lines works for every tool.
        body = body[:-2] + b"\r\n"
        path.write_bytes(body)

    for i in range(config["files"]):
        write_file(root / f"dir_{i % 32:02}" / f"file_{i:05}.txt", config["file_kib"] * 1024)
    for i in range(config["large_files"]):
        write_file(root / "large" / f"large_{i:03}.txt", config["large_mib"] * 1024 * 1024)
    marker.write_text(json.dumps(config))


def invoke(command, root, capture=False):
    start = time.perf_counter()
    result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
                            stderr=subprocess.PIPE, check=False)
    elapsed = (time.perf_counter() - start) * 1000
    if result.returncode not in (0, 1) or result.stderr:
        raise RuntimeError(f"{command}: exit={result.returncode}, stderr={result.stderr!r}")
    return elapsed, result.stdout.count(b"\n") if capture else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--qfind", type=Path, default=Path("target/release/qfind.exe" if os.name == "nt" else "target/release/qfind"))
    parser.add_argument("--data", type=Path, default=Path("bench-data"))
    parser.add_argument("--output", type=Path, default=Path("bench-results.json"))
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--files", type=int, default=1024)
    parser.add_argument("--file-kib", type=int, default=64)
    parser.add_argument("--large-files", type=int, default=4)
    parser.add_argument("--large-mib", type=int, default=16)
    args = parser.parse_args()
    if min(args.runs, args.files, args.file_kib, args.large_mib) < 1 or args.large_files < 0:
        parser.error("Sizes, files, and runs must be positive; --large-files may be zero.")
    root = args.data.resolve()
    config = {k: getattr(args, k) for k in ("files", "file_kib", "large_files", "large_mib")}
    corpus(root, config)
    size = sum(path.stat().st_size for path in root.rglob("*.txt"))
    qfind = args.qfind.resolve()
    if not qfind.is_file():
        parser.error(f"Build the release binary first: {qfind}")
    tools = {"qfind": [str(qfind), "--all", "-g", "*.txt"]}
    if rg := shutil.which("rg"):
        tools["rg"] = [rg, "--no-config", "--hidden", "--no-ignore", "-n", "--column", "-F", "-g", "*.txt"]
    if os.name == "nt" and (findstr := shutil.which("findstr")):
        tools["findstr"] = [findstr, "/S", "/N", "/L"]
    report = {
        "platform": platform.platform(), "cpu_count": os.cpu_count(),
        "cache": "warm (one full correctness run before timing)",
        "dataset": config | {"bytes": size}, "runs": args.runs,
        "qfind_version": subprocess.check_output([str(qfind), "--version"], text=True).strip(),
        "rg_version": subprocess.check_output([rg, "--version"], text=True).splitlines()[0] if rg else None,
        "cases": {},
    }
    rng = random.Random(42)
    print(f"Corpus: {size / 1024**2:.1f} MiB, {args.files + args.large_files} files", flush=True)
    for case, pattern in PATTERNS.items():
        commands = {name: cmd + ([f"/C:{pattern}", "*.txt"] if name == "findstr" else ["--", pattern, "."])
                    for name, cmd in tools.items()}
        counts = {name: invoke(cmd, root, capture=True)[1] for name, cmd in commands.items()}
        if len(set(counts.values())) != 1:
            raise RuntimeError(f"Result count mismatch in {case}: {counts}")
        samples = {name: [] for name in commands}
        for _ in range(args.runs):
            order = list(commands)
            rng.shuffle(order)
            for name in order:
                samples[name].append(invoke(commands[name], root)[0])
        report["cases"][case] = {}
        for name, values in samples.items():
            report["cases"][case][name] = {
                "command": commands[name], "matched_lines": counts[name],
                "median_ms": round(statistics.median(values), 3),
                "min_ms": round(min(values), 3), "max_ms": round(max(values), 3),
                "samples_ms": [round(value, 3) for value in values],
            }
            print(f"{case:8} {name:8} {statistics.median(values):9.2f} ms  matches={counts[name]}", flush=True)
    args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"Saved: {args.output}")


if __name__ == "__main__":
    main()
