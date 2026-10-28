"""Build isolated qfind variants and measure individual regression hypotheses."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import re
import shutil
import statistics
import subprocess

from compare_versions import invoke, records, canonical, binary_info


def replace_once(text, before, after):
    if text.count(before) != 1:
        raise RuntimeError(f"Expected one occurrence: {before[:100]!r}")
    return text.replace(before, after, 1)


def cache_paths(sources):
    text = sources["output.rs"]
    text = replace_once(text, "    path_style: PathStyle,\n    buffer:", "    path_style: PathStyle,\n    path_cache: Option<(std::path::PathBuf, std::sync::Arc<str>)>,\n    buffer:")
    text = replace_once(text, "            json: args.json,", "            json: args.json,\n            path_cache: None,")
    # Both file records and line records use the same per-worker last-file cache.
    text = text.replace("let display = paths::display(path, &self.shared.cwd, self.path_style);", "let display = self.display_path(path);")
    text = text.replace("record.path = display.into();", "record.path = display.to_string().into();")
    text = text.replace("path: display.as_str().into(),", "path: display.as_ref().into(),")
    helper = '''    fn display_path(&mut self, path: &Path) -> std::sync::Arc<str> {
        if let Some((cached, value)) = &self.path_cache
            && cached == path { return value.clone(); }
        let value: std::sync::Arc<str> = paths::display(path, &self.shared.cwd, self.path_style).into();
        self.path_cache = Some((path.to_path_buf(), value.clone()));
        value
    }

'''
    sources["output.rs"] = replace_once(text, "    fn commit(&mut self,", helper + "    fn commit(&mut self,")


def reuse_buffer(sources):
    text = sources["output.rs"]
    text = replace_once(text, "    path_style: PathStyle,", "    path_style: PathStyle,\n    scratch: Vec<u8>,")
    text = replace_once(text, "            json: args.json,", "            json: args.json,\n            scratch: Vec::with_capacity(512),")
    text = replace_once(text, "                let mut encoded = serde_json::to_vec(&record).unwrap();\n                encoded.push(b'\\n');",
                        "                self.scratch.clear();\n                serde_json::to_writer(&mut self.scratch, &record).unwrap();\n                self.scratch.push(b'\\n');")
    text = replace_once(text, "encoded.len() as u64 <= **b", "self.scratch.len() as u64 <= **b")
    text = replace_once(text, "break Some(encoded);", "break Some(std::mem::take(&mut self.scratch));")
    text = replace_once(text, "                    self.buffer.extend_from_slice(&encoded);\n                }\n            } else {",
                        "                    self.buffer.extend_from_slice(&encoded);\n                }\n                self.scratch = encoded;\n            } else {")
    sources["output.rs"] = text


def direct_json(sources):
    text = sources["output.rs"]
    text = replace_once(text, "                let mut encoded = serde_json::to_vec(&record).unwrap();", '''                if budget.is_none() {
                    serde_json::to_writer(&mut self.buffer, &record).unwrap();
                    self.buffer.push(b'\\n');
                    self.commit(start, is_result);
                    return;
                }
                let mut encoded = serde_json::to_vec(&record).unwrap();''')
    sources["output.rs"] = text


def no_ranges(sources):
    text = sources["output.rs"]
    for before in ("end_column: range.as_ref().map(|r| r.end + 1)",
                   "byte_start: Some(offset + range.as_ref().map_or(0, |r| r.start as u64))"):
        text = replace_once(text, before, before.split(":", 1)[0] + ": None")
    text, count = re.subn(r"byte_end: Some\(\s*offset \+ range\.as_ref\(\)\.map_or\(original_len as u64, \|r\| r.end as u64\),\s*\)", "byte_end: None", text)
    if count != 1:
        raise RuntimeError("Could not remove byte_end field")
    sources["output.rs"] = text


def single_regex(sources):
    text = sources["search.rs"]
    needle = "            Self::Regex { regexes, word } => regexes"
    text = replace_once(text, needle, '''            Self::Regex { regexes, word: false } if regexes.len() == 1 =>
                regexes[0].find(text).map(|m| m.range()),
''' + needle)
    sources["search.rs"] = text


def timeout_fast(sources):
    text = sources["main.rs"]
    begin = text.index("    fn stopped(&self) -> bool {")
    end = text.index("    fn stop_for(", begin)
    text = text[:begin] + '''    fn stopped(&self) -> bool {
        if self.timeout.is_some_and(|timeout| self.started.elapsed() >= timeout) {
            self.stop_for(TIME_LIMIT);
        }
        self.stop.load(Ordering::Relaxed)
    }

''' + text[end:]
    sources["main.rs"] = text


def inline_find(sources):
    sources["search.rs"] = replace_once(sources["search.rs"], "    fn find(&self, text:", "    #[inline(always)]\n    fn find(&self, text:")


def regex_hot_loop(sources):
    text = sources["search.rs"]
    before = """                if let Some(range) = self.matcher.as_ref().unwrap().find(text)
                    && !self.matched(path, *line, offset + start as u64, range, text, count)"""
    after = """                let range = match self.matcher.as_ref().unwrap() {
                    Matcher::Regex { regexes, word: false } if regexes.len() == 1 => regexes[0].find(text).map(|m| m.range()),
                    matcher => matcher.find(text),
                };
                if let Some(range) = range
                    && !self.matched(path, *line, offset + start as u64, range, text, count)"""
    sources["search.rs"] = replace_once(text, before, after)


def single_storage(sources):
    text = sources["search.rs"]
    text = replace_once(text, "    Regex { regexes: Vec<Regex>, word: bool },", "    Regex { regexes: Vec<Regex>, word: bool },\n    SingleRegex(Regex),")
    text = replace_once(text, "            let regexes = expressions", "            let mut regexes = expressions")
    text = replace_once(text, "            Ok(Self::Regex {", "            if regexes.len() == 1 && !args.word_regexp { return Ok(Self::SingleRegex(regexes.pop().unwrap())); }\n            Ok(Self::Regex {")
    text = replace_once(text, "            Self::Regex { regexes, word } => regexes", "            Self::SingleRegex(regex) => regex.find(text).map(|m| m.range()),\n            Self::Regex { regexes, word } => regexes")
    before = """                if let Some(range) = self.matcher.as_ref().unwrap().find(text)
                    && !self.matched(path, *line, offset + start as u64, range, text, count)"""
    after = """                let range = match self.matcher.as_ref().unwrap() {
                    Matcher::SingleRegex(regex) => regex.find(text).map(|m| m.range()),
                    matcher => matcher.find(text),
                };
                if let Some(range) = range
                    && !self.matched(path, *line, offset + start as u64, range, text, count)"""
    sources["search.rs"] = replace_once(text, before, after)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=15)
    parser.add_argument("--variants", help="Comma-separated isolated variants to build and measure")
    parser.add_argument("--reuse-builds", action="store_true")
    parser.add_argument("--native-paths", action="store_true", help="Also measure the production binary with --path-style native")
    parser.add_argument("--output", type=Path, default=Path("bench-regression-results.json"))
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    root = Path(__file__).resolve().parent.parent
    base = root / "target" / "regression-variants"
    base.mkdir(parents=True, exist_ok=True)
    target = root / "target" / "regression-build"
    variants = {"cache_paths": [cache_paths], "reuse_buffer": [reuse_buffer], "direct_json": [direct_json],
                "no_ranges": [no_ranges], "single_regex": [single_regex], "timeout_fast": [timeout_fast],
                "combined": [cache_paths, direct_json, single_regex, timeout_fast],
                "rebuilt_current": [], "inline_find": [inline_find], "regex_hot_loop": [regex_hot_loop],
                "single_storage": [single_storage]}
    if args.variants:
        selected = args.variants.split(",")
        if any(name not in variants for name in selected):
            parser.error("Unknown --variants entry")
        variants = {name: variants[name] for name in selected}
    originals = {p.name: p.read_text(encoding="utf-8") for p in (root / "src").glob("*.rs")}
    hashes = {name: hashlib.sha256(text.encode()).hexdigest() for name, text in originals.items()}
    binaries = {"old": Path("C:/utils/qfind.exe"), "current": root / "target/release/qfind.exe"}
    for name, changes in variants.items():
        work = base / name
        if not work.resolve().is_relative_to((root / "target").resolve()):
            raise RuntimeError("Variant escaped target directory")
        (work / "src").mkdir(parents=True, exist_ok=True)
        sources = originals.copy()
        for change in changes:
            change(sources)
        binary = work / "qfind.exe"
        if args.reuse_builds and binary.is_file() and all((work / "src" / filename).is_file() and (work / "src" / filename).read_text(encoding="utf-8") == text for filename, text in sources.items()):
            print(f"Reusing isolated variant: {name}", flush=True)
            binaries[name] = binary
            continue
        for filename, text in sources.items():
            (work / "src" / filename).write_text(text, encoding="utf-8")
        for filename in ("Cargo.toml", "Cargo.lock"):
            shutil.copy2(root / filename, work / filename)
        print(f"Building isolated variant: {name}", flush=True)
        subprocess.run(["cargo", "build", "--release", "--locked", "--manifest-path", str(work / "Cargo.toml"), "--target-dir", str(target)], check=True)
        shutil.copy2(target / "release/qfind.exe", binary)
        binaries[name] = binary

    if args.native_paths:
        binaries["native_paths"] = binaries["current"]

    cases = []
    for name, mode, flags, pattern, threads in [
        ("json_dense_12t", "json", ["--json"], "QF_DENSE_e27a", 12),
        ("json_dense_1t", "json", ["--json"], "QF_DENSE_e27a", 1),
        ("regex_dense_12t", "match", ["-r"], r"QF_DENSE_[a-f0-9]+", 12),
        ("regex_dense_1t", "match", ["-r"], r"QF_DENSE_[a-f0-9]+", 1),
        ("regex_count_1t", "count", ["-r", "-c"], r"QF_DENSE_[a-f0-9]+", 1),
        ("text_dense_12t", "match", [], "QF_DENSE_e27a", 12),
    ]:
        cases.append({"name": name, "cwd": str(root / "bench-data"), "args": ["--all", "-g", "*.txt", "-j", str(threads), *flags, "--", pattern, "."], "mode": mode, "exit": 0,
                      "native_paths_extra": ["--path-style", "native"] if mode == "json" else []})
    result = {"runs": args.runs, "source_sha256": hashes, "binaries": {k: binary_info(p) for k, p in binaries.items()}, "cases": {}}
    print("Validating semantic equivalence and warming each binary...", flush=True)
    for case in cases:
        reference = None
        output_sizes = {}
        for name, exe in binaries.items():
            _, raw = invoke(exe, case, name, capture=True)
            actual = canonical(records(raw, case["mode"]))
            if reference is None:
                reference = actual
            elif actual != reference:
                raise RuntimeError(f"Variant changed results: {name}/{case['name']}")
            output_sizes[name] = len(raw)
            for _ in range(2):
                invoke(exe, case, name)
        result["cases"][case["name"]] = {"args": case["args"], "cwd": case["cwd"], "stdout_bytes": output_sizes,
            "samples": {name: [] for name in binaries}}
    rng = random.Random(20261029)
    for index in range(args.runs):
        order = cases.copy()
        rng.shuffle(order)
        for case in order:
            versions = list(binaries)
            rng.shuffle(versions)
            for name in versions:
                elapsed, _ = invoke(binaries[name], case, name)
                result["cases"][case["name"]]["samples"][name].append(elapsed)
        print(f"Ablation round {index + 1}/{args.runs}", flush=True)
    for name, case in result["cases"].items():
        case["medians_ms"] = {version: statistics.median(values) for version, values in case["samples"].items()}
        print(name + " " + json.dumps({k: round(v, 3) for k, v in case["medians_ms"].items()}), flush=True)
    if hashes != {p.name: hashlib.sha256(p.read_text(encoding="utf-8").encode()).hexdigest() for p in (root / "src").glob("*.rs")}:
        raise RuntimeError("Production source changed during analysis")
    args.output.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(f"Saved {args.output}", flush=True)


if __name__ == "__main__":
    main()
