"""Compare two qfind executables, validating results before randomized warm-cache timing."""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import random
import re
import statistics
import subprocess
import os
import time

from benchmark import corpus, PATTERNS


def binary_info(path):
    return {"path": str(path), "bytes": path.stat().st_size,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "version": subprocess.check_output([str(path), "--version"], text=True).strip()}


def invoke(exe, case, which, capture=False):
    command = [str(exe), *case.get(which + "_extra", []), *case["args"]]
    start = time.perf_counter_ns()
    result = subprocess.run(command, cwd=case["cwd"], stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
                            stderr=subprocess.PIPE, check=False)
    elapsed = (time.perf_counter_ns() - start) / 1_000_000
    if result.returncode != case["exit"] or result.stderr:
        raise RuntimeError(f"{command}: exit={result.returncode}, stderr={result.stderr!r}")
    return elapsed, result.stdout


def normalize(path):
    return path.replace("\\", "/").removeprefix("./")


def records(raw, mode):
    rows = []
    for line in raw.decode("utf-8").splitlines():
        if mode == "json":
            row = json.loads(line)
            if row["type"] == "summary":
                continue
            row = {key: row[key] for key in ("type", "path", "line", "column", "text", "count") if key in row}
        elif mode == "match":
            match = re.fullmatch(r"(.+?):(\d+):(\d+):(.*)", line)
            if not match:
                raise RuntimeError(f"Invalid match output: {line[:200]!r}")
            path, number, column, text = match.groups()
            row = {"type": "match", "path": path, "line": int(number), "column": int(column), "text": text}
        elif mode == "count":
            path, count = line.rsplit(":", 1)
            row = {"type": "count", "path": path, "count": int(count)}
        else:
            row = {"type": "file", "path": line}
        row["path"] = normalize(row["path"])
        rows.append(row)
    return rows


def canonical(rows):
    return Counter(json.dumps(row, sort_keys=True, ensure_ascii=False) for row in rows)


def long_lines(root):
    marker = root / ".qfind-version-benchmark.json"
    config = {"files": 8, "bytes_per_file": 2 * 1024 * 1024, "pattern": "QF_LONG_d63c"}
    if marker.exists():
        if json.loads(marker.read_text()) != config:
            raise RuntimeError("Long-line dataset settings differ")
        if any(not (root / f"long-{i}.txt").is_file() or (root / f"long-{i}.txt").stat().st_size != config["bytes_per_file"] for i in range(8)):
            raise RuntimeError("Long-line dataset is incomplete")
        return config
    if root.exists() and any(root.iterdir()):
        raise RuntimeError("Refusing to overwrite a nonempty long-line dataset")
    root.mkdir(parents=True, exist_ok=True)
    tail = (config["pattern"] + "\r\n").encode()
    body = b"x" * (config["bytes_per_file"] - len(tail)) + tail
    for i in range(8):
        (root / f"long-{i}.txt").write_bytes(body)
    marker.write_text(json.dumps(config))
    return config


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old", type=Path, default=Path("C:/utils/qfind.exe"))
    parser.add_argument("--new", type=Path, default=Path("target/release/qfind.exe"))
    parser.add_argument("--runs", type=int, default=21)
    parser.add_argument("--output", type=Path, default=Path("bench-version-results.json"))
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    root = Path(__file__).resolve().parent.parent
    binaries = {"old": args.old.resolve(), "new": args.new.resolve()}
    for path in binaries.values():
        if not path.is_file():
            parser.error(f"Executable not found: {path}")
    data = root / "bench-data"
    config = {"files": 1024, "file_kib": 64, "large_files": 4, "large_mib": 16}
    corpus(data, config)
    long_root = root / "bench-version-data" / "long-lines"
    long_config = long_lines(long_root)
    common = ["--all", "-g", "*.txt"]
    cases = []

    def add(name, flags, pattern=None, cwd=data, paths=None, mode="match", expected_exit=0, **extra):
        positional = [pattern] if pattern is not None else []
        cases.append({"name": name, "cwd": str(cwd), "args": [*common, *flags, "--", *positional, *(paths or ["."])],
                      "mode": mode, "exit": expected_exit, **extra})

    for name, pattern in PATTERNS.items():
        add("text_" + name, [], pattern, expected_exit=1 if name == "missing" else 0)
    for name, pattern in PATTERNS.items():
        add("json_" + name, ["--json"], pattern, mode="json", expected_exit=1 if name == "missing" else 0)
    add("json_limit100", ["--json", "--max-results", "100"], PATTERNS["dense"], mode="json", subset=True)
    add("regex_dense", ["-r"], r"QF_DENSE_[a-f0-9]+")
    add("ignore_case_dense", ["-i"], PATTERNS["dense"].lower())
    add("count_dense", ["-c"], PATTERNS["dense"], mode="count")
    add("files_with_matches", ["-l"], PATTERNS["dense"], mode="file")
    add("files_listing", ["--files"], mode="file")
    add("quiet_missing", ["-q"], PATTERNS["missing"], mode="file", expected_exit=1)
    add("quiet_dense", ["-q"], PATTERNS["dense"], mode="file")
    small_dirs = [f"dir_{i:02}" for i in range(32)]
    add("small_files_missing", [], PATTERNS["missing"], paths=small_dirs)
    cases[-1]["exit"] = 1
    add("single_large_missing", [], PATTERNS["missing"], paths=["large/large_000.txt"], expected_exit=1)
    # Source files: use the same include filter in both executables.
    add("project_text", [], "fn ", cwd=root, paths=["src", "tests"])
    cases[-1]["args"][2] = "*.rs"
    add("project_json", ["--json"], "fn ", cwd=root, paths=["src", "tests"], mode="json")
    cases[-1]["args"][2] = "*.rs"
    add("long_json_full", ["--json"], long_config["pattern"], cwd=long_root, mode="json", new_extra=["--max-line-bytes", "0"])
    add("long_json_default", ["--json"], long_config["pattern"], cwd=long_root, mode="json", excerpts=True)

    report = {"measured_at_utc": datetime.now(timezone.utc).isoformat(), "platform": platform.platform(),
              "cpu_count": os.cpu_count(), "runs": args.runs, "warmups": 2, "seed": 20261029,
              "cache": "warm; correctness capture and two untimed runs per case/binary",
              "sink": "null device; timing includes process startup, walking, reading, matching, serialization and output",
              "dataset": config | {"bytes": sum(p.stat().st_size for p in data.rglob("*.txt"))},
              "long_lines": long_config, "binaries": {k: binary_info(p) for k, p in binaries.items()}, "cases": {}}
    dense_reference = None
    print("Validating results and warming caches...", flush=True)
    for case in cases:
        captured = {which: invoke(exe, case, which, capture=True)[1] for which, exe in binaries.items()}
        parsed = {which: records(raw, case["mode"]) for which, raw in captured.items()}
        old, new = parsed["old"], parsed["new"]
        if case.get("subset"):
            if len(old) != 100 or len(new) != 100 or any(canonical(rows) - dense_reference for rows in parsed.values()):
                raise RuntimeError(f"Invalid limited results: {case['name']}")
        elif case.get("excerpts"):
            originals = {(r["path"], r["line"], r["column"]): r["text"] for r in old}
            actual_rows = [json.loads(line) for line in captured["new"].splitlines()]
            matches = [r for r in actual_rows if r["type"] == "match"]
            if len(matches) != len(originals):
                raise RuntimeError("Long-line record count differs")
            for row in matches:
                original = originals[(normalize(row["path"]), row["line"], row["column"])].encode()
                snippet = row["text"].encode()
                start = row["text_start_byte"]
                if not row["text_truncated"] or original[start:start + len(snippet)] != snippet or row["text_original_bytes"] != len(original):
                    raise RuntimeError("Invalid long-line excerpt")
        elif canonical(old) != canonical(new):
            raise RuntimeError(f"Search results differ: {case['name']}")
        if case["name"] == "json_dense":
            dense_reference = canonical(old)
        for which, exe in binaries.items():
            for _ in range(2):
                invoke(exe, case, which)
        report["cases"][case["name"]] = {"args": case["args"], "cwd": case["cwd"],
            "old_extra": case.get("old_extra", []), "new_extra": case.get("new_extra", []),
            "correctness": "validated excerpt and location; payload intentionally differs" if case.get("excerpts") else "equal normalized results" if not case.get("subset") else "100 valid matches per binary; subset may differ",
            "old_records": len(old), "new_records": len(new),
            "old_stdout_bytes": len(captured["old"]), "new_stdout_bytes": len(captured["new"]),
            "old_samples_ms": [], "new_samples_ms": []}
        print(f"  {case['name']}: {len(old)} records, bytes {len(captured['old'])}/{len(captured['new'])}", flush=True)

    rng = random.Random(report["seed"])
    for round_index in range(args.runs):
        order = list(cases)
        rng.shuffle(order)
        for case in order:
            versions = list(binaries)
            rng.shuffle(versions)
            for which in versions:
                elapsed, _ = invoke(binaries[which], case, which)
                report["cases"][case["name"]][which + "_samples_ms"].append(elapsed)
        if (round_index + 1) % 3 == 0 or round_index + 1 == args.runs:
            print(f"Timing round {round_index + 1}/{args.runs} complete", flush=True)
    for name, case in report["cases"].items():
        for which in binaries:
            values = case[which + "_samples_ms"]
            case[which] = {"median_ms": statistics.median(values), "min_ms": min(values), "max_ms": max(values),
                           "p90_ms": sorted(values)[min(len(values) - 1, int(len(values) * .9))]}
        case["change_pct"] = (case["new"]["median_ms"] / case["old"]["median_ms"] - 1) * 100
        case["paired_median_ratio"] = statistics.median(n / o for o, n in zip(case["old_samples_ms"], case["new_samples_ms"]))
        print(f"{name:24} old={case['old']['median_ms']:8.2f} new={case['new']['median_ms']:8.2f} ms change={case['change_pct']:+7.1f}%", flush=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"Saved {args.output}", flush=True)


if __name__ == "__main__":
    main()
