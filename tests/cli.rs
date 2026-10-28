use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_qfind"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &Output) -> Vec<Value> {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut rows = json_records(output);
    assert_eq!(rows.last().unwrap()["type"], "summary");
    rows.pop();
    assert!(rows.iter().all(|row| row["type"] != "summary"));
    rows
}

fn json_records(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn literal_is_default_and_results_include_byte_columns() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("한글.txt"),
        "skip\r\n한글 a.b a.b\r\naxb\nlast a.b",
    )
    .unwrap();
    let rows = json(&run(dir.path(), &["--json", "a.b"]));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["line"], 2);
    assert_eq!(rows[0]["column"], 8); // UTF-8 byte offset, not character offset.
    assert_eq!(rows[0]["text"], "한글 a.b a.b");
    assert_eq!(rows[1]["line"], 4);
    assert_eq!(rows[1]["column"], 6);
}

#[test]
fn regex_is_line_based_and_case_folding_is_unicode_aware() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), "ÄPFEL\r\na.b\naxb\n").unwrap();
    let rows = json(&run(dir.path(), &["--json", "-r", "^a.b$"]));
    assert_eq!(rows.len(), 2);
    let rows = json(&run(dir.path(), &["--json", "-i", "äpfel"]));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["line"], 1);
    assert_eq!(
        run(dir.path(), &["-r", r"ÄPFEL\na.b"]).status.code(),
        Some(1)
    );
}

#[test]
fn all_result_modes_and_exit_codes() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), "hit hit\nhit\nmiss\n").unwrap();
    fs::write(dir.path().join("b.txt"), "miss\n").unwrap();
    let counts = json(&run(dir.path(), &["--json", "-c", "hit"]));
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0]["type"], "count");
    assert_eq!(counts[0]["count"], 2);
    let files = json(&run(dir.path(), &["--json", "-l", "hit"]));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["type"], "file");
    let quiet = run(dir.path(), &["-q", "hit"]);
    assert_eq!(quiet.status.code(), Some(0));
    assert!(quiet.stdout.is_empty());
    assert_eq!(run(dir.path(), &["absent"]).status.code(), Some(1));
    assert_eq!(
        run(dir.path(), &["hit", "missing-file"]).status.code(),
        Some(2)
    );
    assert_eq!(run(dir.path(), &["-r", "["]).status.code(), Some(2));
    assert_eq!(
        run(dir.path(), &["--max-results", "0", "hit"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(dir.path(), &["--threads", "0", "hit"]).status.code(),
        Some(2)
    );
    assert_eq!(run(dir.path(), &["x\ny"]).status.code(), Some(2));
}

#[test]
fn ignores_hidden_builtins_and_explicit_paths() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join("node_modules")).unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(dir.path().join(".qfindignore"), "custom.txt\n").unwrap();
    for name in [
        "visible.txt",
        "ignored.txt",
        "custom.txt",
        ".hidden.txt",
        "node_modules/dep.txt",
    ] {
        fs::write(dir.path().join(name), "needle\n").unwrap();
    }
    assert_eq!(json(&run(dir.path(), &["--json", "needle"])).len(), 1);
    assert_eq!(
        json(&run(dir.path(), &["--json", "--hidden", "needle"])).len(),
        2
    );
    assert_eq!(
        json(&run(dir.path(), &["--json", "--no-ignore", "needle"])).len(),
        4
    );
    assert_eq!(
        json(&run(dir.path(), &["--json", "--all", "needle"])).len(),
        5
    );
    assert_eq!(
        json(&run(dir.path(), &["--json", "needle", "ignored.txt"])).len(),
        1
    );
    assert_eq!(
        json(&run(dir.path(), &["--json", "needle", "node_modules"])).len(),
        1
    );
}

#[test]
fn globs_file_listing_and_multiple_roots() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "needle\n").unwrap();
    fs::write(dir.path().join("src/lib.rs"), "needle\n").unwrap();
    fs::write(dir.path().join("data.txt"), "needle\n").unwrap();
    let rows = json(&run(
        dir.path(),
        &["--json", "-g", "*.rs", "-g", "!lib.rs", "needle"],
    ));
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["path"].as_str().unwrap().ends_with("main.rs"));
    let rows = json(&run(dir.path(), &["--json", "--files", "src"]));
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r["type"] == "file"));
    let rows = json(&run(
        dir.path(),
        &["--json", "needle", "src/main.rs", "data.txt"],
    ));
    assert_eq!(rows.len(), 2);
    assert_eq!(
        json(&run(
            dir.path(),
            &["--json", "--files", "src/main.rs", "data.txt"]
        ))
        .len(),
        2
    );
}

#[test]
fn binary_sample_and_invalid_utf8_json() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("binary.dat"), b"\0needle\n").unwrap();
    assert_eq!(run(dir.path(), &["needle"]).status.code(), Some(1));
    assert_eq!(
        json(&run(dir.path(), &["--json", "--text", "needle"])).len(),
        1
    );
    fs::write(dir.path().join("bytes.txt"), b"\xffneedle\n").unwrap();
    let rows = json(&run(dir.path(), &["--json", "needle"]));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["column"], 2);
    assert_eq!(rows[0]["text_base64"], "/25lZWRsZQ==");
}

#[test]
fn empty_pattern_empty_file_crlf_and_no_trailing_newline() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("empty"), b"").unwrap();
    fs::write(dir.path().join("lines"), b"\r\na\nlast").unwrap();
    let rows = json(&run(dir.path(), &["--json", ""]));
    assert_eq!(rows.len(), 3);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["line"], i + 1);
        assert_eq!(row["column"], 1);
    }
    assert_eq!(rows[0]["text"], "");
    assert_eq!(json(&run(dir.path(), &["--json", "-r", "^"])).len(), 3);
}

#[test]
fn carriage_returns_are_searched_after_crlf_normalization_in_every_mode() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("crlf.txt"), b"first\r\nsecond\r\n").unwrap();
    for mode in [None, Some("-l"), Some("-c"), Some("-q")] {
        let mut args = vec!["\r", "crlf.txt"];
        if let Some(mode) = mode {
            args.insert(0, mode);
        }
        let output = run(dir.path(), &args);
        assert_eq!(output.status.code(), Some(1), "mode={mode:?}");
        assert!(output.stdout.is_empty());
    }

    fs::write(dir.path().join("cr.txt"), b"\r\nleft\rright\r\nlast\r").unwrap();
    let rows = json(&run(dir.path(), &["--json", "\r", "cr.txt"]));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["line"], 2);
    assert_eq!(rows[0]["column"], 5);
    assert_eq!(rows[0]["text"], "left\rright");
    assert_eq!(rows[1]["line"], 3);
    assert_eq!(rows[1]["column"], 5);
    assert_eq!(rows[1]["text"], "last\r");
    assert_eq!(
        rows,
        json(&run(dir.path(), &["--json", "-r", r"\r", "cr.txt"]))
    );
    let counts = json(&run(dir.path(), &["--json", "-c", "\r", "cr.txt"]));
    assert_eq!(counts[0]["count"], 2);
    assert_eq!(
        json(&run(dir.path(), &["--json", "-l", "\r", "cr.txt"])).len(),
        1
    );
    assert_eq!(
        run(dir.path(), &["-q", "\r", "cr.txt"]).status.code(),
        Some(0)
    );
}

#[test]
fn long_lines_and_matches_across_read_boundaries() {
    let dir = TempDir::new().unwrap();
    let mut bytes = b"before\n".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', 256 * 1024 - 10));
    bytes.extend_from_slice(b"needle\r\nnext needle\n");
    bytes.extend(std::iter::repeat_n(b'z', 2 * 1024 * 1024));
    bytes.extend_from_slice(b"needle");
    fs::write(dir.path().join("long.txt"), bytes).unwrap();
    let rows = json(&run(dir.path(), &["--json", "needle"]));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["line"], 2);
    assert_eq!(rows[0]["column"], 256 * 1024 - 9);
    assert_eq!(rows[1]["line"], 3);
    assert_eq!(rows[1]["column"], 6);
    assert_eq!(rows[2]["line"], 4);
    assert_eq!(rows[2]["column"], 2 * 1024 * 1024 + 1);
    let regex_rows = json(&run(dir.path(), &["--json", "-r", "needle"]));
    assert_eq!(rows, regex_rows);
}

#[test]
fn global_limit_is_exact_under_parallel_contention() {
    let dir = TempDir::new().unwrap();
    for i in 0..80 {
        fs::write(dir.path().join(format!("{i}.txt")), "hit\n".repeat(100)).unwrap();
    }
    for mode in [None, Some("-l"), Some("-c")] {
        let mut args = vec!["--json", "-j", "8", "--max-results", "17"];
        if let Some(mode) = mode {
            args.push(mode);
        }
        args.push("hit");
        let rows = json(&run(dir.path(), &args));
        assert_eq!(rows.len(), 17);
        if mode == Some("-c") {
            assert!(rows.iter().all(|r| r["count"] == 100));
        }
    }
    assert_eq!(
        json(&run(
            dir.path(),
            &["--json", "--files", "--max-results", "17"]
        ))
        .len(),
        17
    );
}

#[test]
fn stats_stay_on_stderr_and_broken_pipe_does_not_panic() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("many.txt"), "needle\n".repeat(30_000)).unwrap();
    let output = run(
        dir.path(),
        &["--json", "--stats", "--max-results", "1", "needle"],
    );
    assert_eq!(json(&output).len(), 1);
    let stats: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(stats["type"], "stats");
    assert!(stats["matched_lines"].is_number());
    let mut child = Command::new(env!("CARGO_BIN_EXE_qfind"))
        .current_dir(dir.path())
        .arg("needle")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdout.take().unwrap();
    let mut first_byte = [0];
    pipe.read_exact(&mut first_byte).unwrap();
    drop(pipe);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn deterministic_generated_corpus_matches_reference_line_search() {
    let dir = TempDir::new().unwrap();
    let mut seed = 42_u64;
    let mut text = String::new();
    let mut expected = Vec::new();
    for line in 1..=20_000 {
        let mut content = String::new();
        for _ in 0..80 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            content.push((b'a' + ((seed >> 32) % 8) as u8) as char);
        }
        if let Some(column) = content.find("abc") {
            expected.push((line, column + 1, content.clone()));
        }
        text.push_str(&content);
        text.push_str(if line % 3 == 0 { "\r\n" } else { "\n" });
    }
    fs::write(dir.path().join("generated.txt"), text).unwrap();
    let rows = json(&run(dir.path(), &["--json", "abc"]));
    assert_eq!(rows.len(), expected.len());
    for (row, (line, column, text)) in rows.iter().zip(expected) {
        assert_eq!(row["line"], line);
        assert_eq!(row["column"], column);
        assert_eq!(row["text"], text);
    }
}

#[test]
fn summaries_distinguish_completion_limits_and_no_matches() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), "hit\nhit\nhit\n").unwrap();
    for (args, code, count, complete, reason) in [
        (vec!["--json", "hit"], 0, 3, true, "completed"),
        (vec!["--json", "absent"], 1, 0, true, "completed"),
        (
            vec!["--json", "--max-results", "2", "hit"],
            0,
            2,
            false,
            "result_limit",
        ),
        // Reaching exactly N also stops early; do not claim there are more results.
        (
            vec!["--json", "--max-results", "3", "hit"],
            0,
            3,
            false,
            "result_limit",
        ),
        (vec!["--json", "--files"], 0, 1, true, "completed"),
    ] {
        let output = run(dir.path(), &args);
        assert_eq!(output.status.code(), Some(code));
        let rows = json_records(&output);
        let summary = rows.last().unwrap();
        assert_eq!(summary["type"], "summary");
        assert_eq!(summary["schema_version"], 1);
        assert_eq!(summary["records_emitted"], count);
        assert_eq!(summary["matched"], count > 0);
        assert_eq!(summary["scan_complete"], complete);
        assert_eq!(summary["stop_reason"], reason);
        assert_eq!(summary["error_count"], 0);
        assert!(summary.get("has_more").is_none());
        assert_eq!(rows.len(), count + 1);
    }
}

#[test]
fn json_errors_cover_arguments_patterns_globs_and_traversal() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), "hit\n").unwrap();
    for (args, code, stage) in [
        (
            vec!["--json", "--threads", "0", "hit"],
            "invalid_arguments",
            "arguments",
        ),
        (
            vec!["--threads", "0", "--json", "hit"],
            "invalid_arguments",
            "arguments",
        ),
        (
            vec!["--json", "--unknown", "hit"],
            "invalid_arguments",
            "arguments",
        ),
        (vec!["--json", "-r", "["], "invalid_pattern", "pattern"),
        (vec!["--json", "-g", "[", "hit"], "invalid_glob", "glob"),
        (
            vec!["--json", "hit", "missing.txt"],
            "traversal_error",
            "traversal",
        ),
    ] {
        let output = run(dir.path(), &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let errors: Vec<Value> = String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(errors.len(), 1, "{args:?}");
        assert_eq!(errors[0]["type"], "error");
        assert_eq!(errors[0]["code"], code);
        assert_eq!(errors[0]["stage"], stage);
        if stage == "traversal" {
            assert!(errors[0]["path"].as_str().unwrap().ends_with("missing.txt"));
            assert!(errors[0]["os_error"].is_number());
        }
        let rows: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["type"], "summary");
        assert_eq!(rows[0]["scan_complete"], false);
        assert_eq!(rows[0]["stop_reason"], "error");
        assert_eq!(rows[0]["error_count"], 1);
    }
}

#[test]
fn partial_results_and_errors_are_reported_together() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), "hit\n").unwrap();
    let output = run(dir.path(), &["--json", "hit", "data.txt", "missing.txt"]);
    assert_eq!(output.status.code(), Some(2));
    let rows = json_records(&output);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["type"], "match");
    assert_eq!(rows[1]["records_emitted"], 1);
    assert_eq!(rows[1]["error_count"], 1);
    assert_eq!(rows[1]["scan_complete"], false);
}

#[test]
fn output_budget_includes_json_escaping_and_summary_under_parallel_contention() {
    let dir = TempDir::new().unwrap();
    for i in 0..80 {
        fs::write(
            dir.path().join(format!("{i}.txt")),
            "hit\t\"\\\u{1}\n".repeat(100),
        )
        .unwrap();
    }
    for mode in [None, Some("-l"), Some("-c"), Some("--files")] {
        let mut args = vec!["--json", "-j", "8", "--max-output-bytes", "2048"];
        if let Some(mode) = mode {
            args.push(mode);
        }
        if mode != Some("--files") {
            args.push("hit");
        }
        let output = run(dir.path(), &args);
        let data = json(&output);
        assert!(!data.is_empty());
        assert!(
            output.stdout.len() <= 2048,
            "{args:?}: {}",
            output.stdout.len()
        );
        let rows = json_records(&output);
        let summary = rows.last().unwrap();
        assert_eq!(summary["records_emitted"], data.len());
        assert_eq!(summary["scan_complete"], false);
        assert_eq!(summary["stop_reason"], "output_limit");
        assert_eq!(summary["error_count"], 0);
    }
}

#[test]
fn an_oversized_record_is_adapted_to_the_remaining_budget() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("data.txt"),
        format!("{}hit\n", "x".repeat(4096)),
    )
    .unwrap();
    let output = run(dir.path(), &["--json", "--max-output-bytes", "1024", "hit"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.len() <= 1024);
    let rows = json_records(&output);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["type"], "match");
    assert_eq!(rows[0]["text_truncated"], true);
    assert_eq!(rows[0]["column"], 4097);
    assert_eq!(rows[1]["records_emitted"], 1);
    assert_eq!(rows[1]["matched"], true);
    assert_eq!(rows[1]["scan_complete"], true);
}

#[test]
fn output_budget_accepts_a_record_at_the_exact_reserved_boundary() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("data.txt"),
        format!("hit{}\nhit{}\n", "x".repeat(600), "x".repeat(600)),
    )
    .unwrap();
    let baseline = run(dir.path(), &["--json", "hit", "data.txt"]);
    let record_bytes = baseline
        .stdout
        .split_inclusive(|b| *b == b'\n')
        .next()
        .unwrap()
        .len();
    for budget in [768 + record_bytes, 767 + record_bytes] {
        let budget_arg = budget.to_string();
        let output = run(
            dir.path(),
            &[
                "--json",
                "--max-output-bytes",
                &budget_arg,
                "hit",
                "data.txt",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stdout.len() <= budget);
        let rows = json_records(&output);
        assert!(rows.len() >= 2);
        let summary = rows.last().unwrap();
        assert_eq!(summary["records_emitted"], rows.len() - 1);
        if budget == 768 + record_bytes {
            assert_eq!(rows[0]["text"].as_str().unwrap().len(), 603);
        } else {
            assert_eq!(rows[0]["text_truncated"], true);
        }
    }
}

#[test]
fn long_line_excerpts_preserve_original_positions_and_utf8() {
    let dir = TempDir::new().unwrap();
    let text = format!("{}needle{}", "한".repeat(1000), "글".repeat(1000));
    fs::write(dir.path().join("data.txt"), format!("{text}\r\n")).unwrap();
    let rows = json(&run(
        dir.path(),
        &["--json", "--max-line-bytes", "100", "needle"],
    ));
    let row = &rows[0];
    let excerpt = row["text"].as_str().unwrap();
    let start = row["text_start_byte"].as_u64().unwrap() as usize;
    assert!(excerpt.len() <= 100);
    assert!(excerpt.contains("needle"));
    assert_eq!(row["line"], 1);
    assert_eq!(row["column"], 3001);
    assert_eq!(row["text_truncated"], true);
    assert_eq!(row["text_original_bytes"], text.len());
    assert_eq!(&text[start..start + excerpt.len()], excerpt);
    assert!(row.get("text_base64").is_none());
    let full = json(&run(
        dir.path(),
        &["--json", "--max-line-bytes", "0", "needle"],
    ));
    assert_eq!(full[0]["text"], text);
    assert!(full[0].get("text_truncated").is_none());
    let default = json(&run(dir.path(), &["--json", "needle"]));
    assert!(default[0]["text"].as_str().unwrap().len() <= 4096);
    assert_eq!(default[0]["text_truncated"], true);
    let tiny = json(&run(dir.path(), &["--json", "--max-line-bytes", "1", "한"]));
    assert!(tiny[0]["text"].as_str().unwrap().len() <= 1);
    assert!(tiny[0].get("text_base64").is_none());
}

#[test]
fn invalid_utf8_excerpts_keep_lossless_bytes_and_original_offset() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let dir = TempDir::new().unwrap();
    let mut text = vec![0xff; 1000];
    text.extend_from_slice(b"needle");
    text.extend_from_slice(&[0xfe; 1000]);
    fs::write(dir.path().join("data.txt"), &text).unwrap();
    let rows = json(&run(
        dir.path(),
        &["--json", "--max-line-bytes", "100", "needle"],
    ));
    let row = &rows[0];
    let bytes = STANDARD
        .decode(row["text_base64"].as_str().unwrap())
        .unwrap();
    let start = row["text_start_byte"].as_u64().unwrap() as usize;
    assert_eq!(bytes.len(), 100);
    assert_eq!(bytes, text[start..start + bytes.len()]);
    assert_eq!(row["column"], 1001);
    assert_eq!(row["text_original_bytes"], text.len());
}

#[test]
fn json_only_limits_are_validated_and_help_and_plain_output_stay_plain() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), "hit\n").unwrap();
    for args in [
        vec!["--max-output-bytes", "1024", "hit"],
        vec!["--max-line-bytes", "100", "hit"],
        vec!["--json", "--max-output-bytes", "1023", "hit"],
    ] {
        assert_eq!(run(dir.path(), &args).status.code(), Some(2));
    }
    for flag in ["--help", "--version"] {
        let output = run(dir.path(), &["--json", flag]);
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("\"type\":\"summary\""));
    }
    let output = run(dir.path(), &["hit", "data.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"data.txt:1:1:hit\n");
    let output = run(dir.path(), &["--", "--json", "missing.txt"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("qfind:"));
}

#[test]
fn multiple_patterns_word_and_whole_line_search_use_literal_defaults() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("data.txt"),
        "a.b\naxb\nneedlework\nneedle needle\n한글\n한글자\n-foo-\n",
    )
    .unwrap();
    let rows = json(&run(
        dir.path(),
        &["--json", "-e", "a.b", "-e", "한글", "data.txt"],
    ));
    assert_eq!(
        rows.iter()
            .map(|r| r["line"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 5, 6]
    );
    let rows = json(&run(
        dir.path(),
        &["--json", "-w", "-e", "needle", "-e", "한글", "data.txt"],
    ));
    assert_eq!(
        rows.iter()
            .map(|r| r["line"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [4, 5]
    );
    assert_eq!(rows[0]["column"], 1);
    assert_eq!(rows[0]["end_column"], 7);
    assert_eq!(rows[1]["end_column"], 7);
    let rows = json(&run(
        dir.path(),
        &["--json", "-w", "--", "-foo-", "data.txt"],
    ));
    assert_eq!(rows[0]["line"], 7);
    let rows = json(&run(
        dir.path(),
        &["--json", "-x", "-e", "a.b", "-e", "한글", "data.txt"],
    ));
    assert_eq!(rows.len(), 2);
    let rows = json(&run(
        dir.path(),
        &["--json", "-r", "-e", "^a.b$", "-e", "^한글$", "data.txt"],
    ));
    assert_eq!(rows.len(), 3);
    let rows = json(&run(
        dir.path(),
        &["--json", "-i", "-w", "NEEDLE", "data.txt"],
    ));
    assert_eq!(rows.len(), 1);
}

#[test]
fn context_merges_overlaps_and_preserves_match_and_context_byte_ranges() {
    let dir = TempDir::new().unwrap();
    let text = "one\r\ntwo\r\nx needle\r\nfour\r\nneedle\r\nsix\r\nseven";
    fs::write(dir.path().join("data.txt"), text).unwrap();
    let output = run(dir.path(), &["--json", "-C", "2", "needle", "data.txt"]);
    let rows = json(&output);
    assert_eq!(rows.len(), 7);
    assert_eq!(
        rows.iter()
            .map(|r| r["line"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        (1..=7).collect::<Vec<_>>()
    );
    assert_eq!(rows[2]["type"], "match");
    assert_eq!(rows[4]["type"], "match");
    assert_eq!(rows[0]["type"], "context");
    assert_eq!(rows[0]["byte_start"], 0);
    assert_eq!(rows[0]["byte_end"], 3);
    let start = text.find("needle").unwrap();
    assert_eq!(rows[2]["byte_start"], start);
    assert_eq!(rows[2]["byte_end"], start + 6);
    assert_eq!(rows[2]["column"], 3);
    assert_eq!(rows[2]["end_column"], 9);
    let summary = json_records(&output).pop().unwrap();
    assert_eq!(summary["context_records_emitted"], 5);
    assert_eq!(summary["records_emitted"], 7);
    assert_eq!(summary["scan_complete"], true);
    let rows = json(&run(
        dir.path(),
        &[
            "--json", "-C", "2", "-B", "0", "-A", "1", "needle", "data.txt",
        ],
    ));
    assert_eq!(
        rows.iter()
            .map(|r| r["line"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [3, 4, 5, 6]
    );
}

#[test]
fn context_does_not_consume_the_match_limit_or_break_the_byte_budget() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("data.txt"),
        "before\nhit\nmiddle\nhit\nafter\n",
    )
    .unwrap();
    let output = run(
        dir.path(),
        &["--json", "-C", "1", "--max-results", "2", "hit"],
    );
    let rows = json(&output);
    assert_eq!(rows.iter().filter(|r| r["type"] == "match").count(), 2);
    assert_eq!(
        rows.iter()
            .map(|r| r["line"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    let summary = json_records(&output).pop().unwrap();
    assert_eq!(summary["context_records_emitted"], 2);
    assert_eq!(summary["stop_reason"], "result_limit");
    let output = run(
        dir.path(),
        &["--json", "-C", "1", "--max-output-bytes", "1024", "hit"],
    );
    assert!(output.stdout.len() <= 1024);
    let rows = json_records(&output);
    assert_eq!(rows.last().unwrap()["records_emitted"], rows.len() - 1);
    assert_eq!(rows.last().unwrap()["stop_reason"], "output_limit");
    let output = run(dir.path(), &["-C", "1", "hit", "data.txt"]);
    assert_eq!(output.stdout, b"data.txt-1-before\ndata.txt:2:1:hit\ndata.txt-3-middle\ndata.txt:4:1:hit\ndata.txt-5-after\n");
}

#[test]
fn sorted_results_are_global_and_reproducible_before_limits() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("a")).unwrap();
    for path in ["z.txt", "b.txt", "a/x.txt", "a.txt"] {
        fs::write(dir.path().join(path), "hit\n").unwrap();
    }
    for _ in 0..3 {
        let rows = json(&run(
            dir.path(),
            &[
                "--json",
                "--sort",
                "path",
                "-j",
                "8",
                "--max-results",
                "2",
                "hit",
            ],
        ));
        assert_eq!(
            rows.iter()
                .map(|r| r["path"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["a.txt", "a/x.txt"]
        );
    }
    let rows = json(&run(dir.path(), &["--json", "--sort", "path", "--files"]));
    assert_eq!(
        rows.iter()
            .map(|r| r["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a.txt", "a/x.txt", "b.txt", "z.txt"]
    );
}

#[test]
fn path_styles_handle_explicit_absolute_roots_and_diagnostics() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    let path = dir.path().join("src/data.txt");
    fs::write(&path, "hit\n").unwrap();
    let abs = path.to_str().unwrap();
    let rows = json(&run(dir.path(), &["--json", "hit", abs]));
    assert_eq!(rows[0]["path"], "src/data.txt");
    let rows = json(&run(
        dir.path(),
        &["--json", "--path-style", "absolute", "hit", "src"],
    ));
    let result = rows[0]["path"].as_str().unwrap();
    assert!(Path::new(result).is_absolute());
    assert!(result.ends_with("/src/data.txt"));
    let missing = dir.path().join("src/missing.txt");
    let output = run(dir.path(), &["--json", "hit", missing.to_str().unwrap()]);
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["path"], "src/missing.txt");
    let rows = json(&run(&dir.path().join("src"), &["--json", "hit", abs]));
    assert_eq!(rows[0]["path"], "data.txt");
}

#[test]
fn file_and_line_caps_report_incomplete_scans_and_do_not_emit_partial_counts() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("small.txt"), "hit\n").unwrap();
    fs::write(dir.path().join("large.txt"), "hit\n".repeat(100)).unwrap();
    let output = run(dir.path(), &["--json", "--max-file-bytes", "4", "hit"]);
    assert_eq!(output.status.code(), Some(2));
    let rows = json_records(&output);
    assert_eq!(rows.len(), 2);
    let summary = rows.last().unwrap();
    assert_eq!(summary["files_visited"], 2);
    assert_eq!(summary["files_searched"], 1);
    assert_eq!(summary["oversized_files_skipped"], 1);
    assert_eq!(summary["scan_complete"], false);
    assert_eq!(summary["stop_reason"], "resource_limit");
    let text = format!("hit\r\n{}hit\r\nhit\r\n", "x".repeat(600_000));
    fs::write(dir.path().join("lines.txt"), &text).unwrap();
    let output = run(
        dir.path(),
        &["--json", "--max-line-buffer-bytes", "3", "hit", "lines.txt"],
    );
    assert_eq!(output.status.code(), Some(2));
    let rows = json_records(&output);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["line"], 1);
    assert_eq!(rows[1]["line"], 3);
    assert_eq!(rows[1]["byte_start"], text.rfind("hit").unwrap());
    assert_eq!(rows[2]["lines_skipped"], 1);
    assert_eq!(rows[2]["scan_complete"], false);
    let output = run(
        dir.path(),
        &[
            "--json",
            "-c",
            "--max-line-buffer-bytes",
            "3",
            "hit",
            "lines.txt",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_records(&output).len(), 1);
}

#[test]
fn line_cap_handles_crlf_boundaries_eof_and_resets_context_across_skips() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), b"hit\r\nlongline\r\nhit\r").unwrap();
    let output = run(
        dir.path(),
        &[
            "--json",
            "-C",
            "1",
            "--max-line-buffer-bytes",
            "3",
            "hit",
            "data.txt",
        ],
    );
    let rows = json_records(&output);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["text"], "hit");
    assert_eq!(rows[1]["lines_skipped"], 2);
    assert_eq!(rows[1]["context_records_emitted"], 0);
    let mut text = vec![b'x'; 256 * 1024 - 1];
    text.extend_from_slice(b"\r\nhit");
    fs::write(dir.path().join("boundary.txt"), text).unwrap();
    let output = run(
        dir.path(),
        &[
            "--json",
            "--max-line-buffer-bytes",
            "262143",
            "hit",
            "boundary.txt",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let rows = json(&output);
    assert_eq!(rows[0]["line"], 2);
    assert_eq!(rows[0]["byte_start"], 262145);
}

#[test]
fn diagnostics_obey_count_and_byte_limits_without_hiding_total_errors() {
    let dir = TempDir::new().unwrap();
    let missing: Vec<_> = (0..100).map(|i| format!("missing-{i}.txt")).collect();
    for (limit, bytes) in [(3, 1024), (0, 1024), (100, 150), (100, 0)] {
        let limit_arg = limit.to_string();
        let bytes_arg = bytes.to_string();
        let mut args = vec![
            "--json",
            "--max-errors",
            &limit_arg,
            "--max-error-bytes",
            &bytes_arg,
            "hit",
        ];
        args.extend(missing.iter().map(String::as_str));
        let output = run(dir.path(), &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.len() <= bytes);
        let errors: Vec<Value> = String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(errors.len() <= limit);
        let rows: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows[0]["error_count"], 100);
        assert_eq!(rows[0]["errors_emitted"], errors.len());
        assert_eq!(rows[0]["errors_suppressed"], 100 - errors.len());
    }
}

#[test]
fn binary_coverage_and_ranges_are_reported_without_marking_expected_exclusions_incomplete() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("binary.dat"), b"\0hit\n").unwrap();
    fs::write(dir.path().join("data.txt"), "한글\r\nxx hit hit\n").unwrap();
    let output = run(dir.path(), &["--json", "hit"]);
    let rows = json(&output);
    assert_eq!(rows[0]["column"], 4);
    assert_eq!(rows[0]["end_column"], 7);
    assert_eq!(rows[0]["byte_start"], 11);
    assert_eq!(rows[0]["byte_end"], 14);
    let summary = json_records(&output).pop().unwrap();
    assert_eq!(summary["files_visited"], 2);
    assert_eq!(summary["files_searched"], 1);
    assert_eq!(summary["binary_files_skipped"], 1);
    assert_eq!(summary["scan_complete"], true);
}

#[test]
fn cooperative_deadline_returns_an_incomplete_summary() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("many.txt"), "hit\n".repeat(1_000_000)).unwrap();
    let output = run(
        dir.path(),
        &["--json", "--timeout-ms", "1", "-j", "1", "-c", "hit"],
    );
    assert_eq!(output.status.code(), Some(2));
    let rows = json_records(&output);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["scan_complete"], false);
    assert_eq!(rows[0]["stop_reason"], "time_limit");
    assert_eq!(rows[0]["error_count"], 0);
}

#[test]
fn new_option_conflicts_and_limits_are_structured_errors() {
    let dir = TempDir::new().unwrap();
    for args in [
        vec!["--json", "--files", "-e", "hit"],
        vec!["--json", "-c", "-C", "1", "hit"],
        vec!["--json", "--files", "-A", "1"],
        vec!["--json", "--timeout-ms", "0", "hit"],
        vec!["--json", "--max-file-bytes", "0", "hit"],
        vec!["--json", "--max-line-buffer-bytes", "0", "hit"],
        vec!["--json", "--sort", "random", "hit"],
        vec!["--json", "--path-style", "unknown", "hit"],
    ] {
        let output = run(dir.path(), &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "invalid_arguments");
        assert_eq!(json_records(&output)[0]["scan_complete"], false);
    }
}

#[test]
fn adaptive_excerpts_account_for_escaped_and_base64_bytes() {
    let dir = TempDir::new().unwrap();
    for bytes in [vec![b'\t'; 10_000], vec![0xff; 10_000]] {
        let mut text = bytes.clone();
        text.extend_from_slice(b"needle");
        text.extend_from_slice(&bytes);
        fs::write(dir.path().join("data.txt"), &text).unwrap();
        let output = run(
            dir.path(),
            &[
                "--json",
                "--max-line-bytes",
                "0",
                "--max-output-bytes",
                "1024",
                "needle",
            ],
        );
        assert!(output.stdout.len() <= 1024);
        let rows = json(&output);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["byte_start"], 10_000);
        assert_eq!(rows[0]["byte_end"], 10_006);
        assert_eq!(rows[0]["text_truncated"], true);
    }
}

#[test]
fn regex_patterns_keep_capture_names_local_and_select_the_earliest_span() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), "bar foo\nfoobar\n").unwrap();
    let rows = json(&run(
        dir.path(),
        &[
            "--json",
            "-r",
            "-e",
            "(?P<symbol>foo)",
            "-e",
            "(?P<symbol>bar)",
        ],
    ));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["column"], 1);
    assert_eq!(rows[0]["end_column"], 4);
    assert_eq!(rows[1]["column"], 1);
    let rows = json(&run(dir.path(), &["--json", "-e", "foo", "-e", "foobar"]));
    assert_eq!(rows[1]["end_column"], 4);
    let rows = json(&run(dir.path(), &["--json", "-e", "foobar", "-e", "foo"]));
    assert_eq!(rows[1]["end_column"], 7);
}
