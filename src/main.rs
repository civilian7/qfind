mod output;
mod paths;
mod search;

use clap::{ArgGroup, Parser, ValueEnum};
use ignore::{WalkBuilder, WalkState, overrides::OverrideBuilder};
use std::ffi::OsString;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use output::{Diagnostic, Output, Summary};
use search::{Matcher, Worker};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Sort {
    Path,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PathStyle {
    Native,
    Relative,
    Absolute,
}

/// 빠른 병렬 파일 내용 검색기. 기본값은 문자열 그대로 검색입니다.
#[derive(Debug, Parser)]
#[command(
    version,
    about,
    after_help = "실행 예시:\n  qfind '검색어' .\n  qfind --json --max-results 100 '검색어' src\n  qfind -r 'fn\\s+\\w+' src\n  qfind -g '*.rs' -g '!generated/**' 'TODO' .\n  qfind --files src\n  qfind -- '--flag' .\n\n출력: 경로:줄:열:내용 (줄과 바이트 열은 1부터 시작)\n기본 제외: 숨김 파일, 무시 규칙, 의존성·빌드 폴더. --all로 포함합니다.\n종료 코드: 0=결과 있음, 1=결과 없음, 2=오류",
    group(ArgGroup::new("mode").args(["count", "files_with_matches", "quiet", "files"]))
)]
struct Args {
    /// 검색할 문자열 (-r 사용 시 정규식); --files·-e 사용 시 첫 검색 경로
    #[arg(required_unless_present_any = ["files", "expressions"])]
    pattern: Option<OsString>,
    /// 검색어 추가 (OR). -e 사용 시 위치 인수는 모두 경로
    #[arg(short = 'e', long = "expression", conflicts_with = "files")]
    expressions: Vec<String>,
    /// 검색할 파일 또는 폴더 [기본값: 현재 폴더]
    paths: Vec<PathBuf>,
    /// 검색어를 정규식으로 해석 (한 줄씩 검색)
    #[arg(short = 'r', long, conflicts_with = "files")]
    regex: bool,
    /// Unicode 규칙으로 대소문자를 구분하지 않고 검색
    #[arg(short = 'i', long, conflicts_with = "files")]
    ignore_case: bool,
    /// Unicode 단어 전체 일치
    #[arg(short = 'w', long, conflicts_with = "files")]
    word_regexp: bool,
    /// 줄 전체 일치
    #[arg(short = 'x', long, conflicts_with = "files")]
    line_regexp: bool,
    /// 일치 전후 문맥 줄 수
    #[arg(short = 'C', long, conflicts_with_all = ["count", "files_with_matches", "quiet", "files"])]
    context: Option<usize>,
    /// 일치 앞 문맥 줄 수 (-C보다 우선)
    #[arg(short = 'B', long, conflicts_with_all = ["count", "files_with_matches", "quiet", "files"])]
    before_context: Option<usize>,
    /// 일치 뒤 문맥 줄 수 (-C보다 우선)
    #[arg(short = 'A', long, conflicts_with_all = ["count", "files_with_matches", "quiet", "files"])]
    after_context: Option<usize>,
    /// 결과마다 한 줄의 JSON 객체 출력 (NDJSON)
    #[arg(long, conflicts_with = "quiet")]
    json: bool,
    /// 일치하는 파일 경로만 출력; 파일별 첫 일치에서 읽기 종료
    #[arg(short = 'l', long)]
    files_with_matches: bool,
    /// 일치하는 파일별로 일치 줄 수 출력
    #[arg(short = 'c', long)]
    count: bool,
    /// 첫 일치에서 종료하고 종료 코드만 반환
    #[arg(short = 'q', long)]
    quiet: bool,
    /// 내용을 읽지 않고 검색 대상 파일 경로만 출력
    #[arg(long)]
    files: bool,
    /// 파일 경로 glob 필터; 반복 지정 가능, 제외 패턴은 !로 시작
    #[arg(short = 'g', long = "glob", allow_hyphen_values = true)]
    globs: Vec<String>,
    /// 숨김 파일과 폴더 포함
    #[arg(long)]
    hidden: bool,
    /// 무시 파일 규칙과 기본 의존성·빌드 폴더 제외 해제
    #[arg(short = 'u', long)]
    no_ignore: bool,
    /// 숨김·무시 파일 모두 포함 (--hidden --no-ignore와 동일)
    #[arg(long)]
    all: bool,
    /// NUL 바이트를 포함한 파일도 텍스트로 검색
    #[arg(short = 'a', long)]
    text: bool,
    /// 심볼릭 링크 따라가기 (순환은 오류로 보고)
    #[arg(short = 'L', long)]
    follow: bool,
    /// 작업자 수, 1~256 [기본값: 사용 가능한 CPU 수, 최대 12]
    #[arg(short = 'j', long, value_parser = clap::value_parser!(u16).range(1..=256))]
    threads: Option<u16>,
    /// 전체 일치 줄·파일 결과 상한 (문맥·요약 제외)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), conflicts_with = "quiet")]
    max_results: Option<u64>,
    /// JSON stdout의 전체 바이트 상한 (요약 포함, 최소 1024)
    #[arg(long, requires = "json", value_parser = clap::value_parser!(u64).range(1024..))]
    max_output_bytes: Option<u64>,
    /// JSON의 줄 내용 발췌 크기 (원본 바이트, 기본 4096, 0=제한 없음)
    #[arg(long, requires = "json")]
    max_line_bytes: Option<u64>,
    /// 협력적 검색 시간 제한 (밀리초, 블로킹 OS I/O는 즉시 중단하지 못함)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_ms: Option<u64>,
    /// 파일별 최대 읽기 크기 (바이트)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), conflicts_with = "files")]
    max_file_bytes: Option<u64>,
    /// 검색할 줄의 최대 원본 바이트 수 (개행 제외, 긴 줄은 건너뜀)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), conflicts_with = "files")]
    max_line_buffer_bytes: Option<u64>,
    /// stderr 오류 레코드 상한 (0=모두 생략)
    #[arg(long, default_value = "20")]
    max_errors: u64,
    /// stderr 오류의 직렬화 바이트 상한 (0=모두 생략, 통계 제외)
    #[arg(long, default_value = "16384")]
    max_error_bytes: u64,
    /// 전체 경로 정렬 후 순차 검색 (제한된 결과도 재현 가능)
    #[arg(long, value_enum)]
    sort: Option<Sort>,
    /// 경로 표현 (기본: JSON=relative, 텍스트=native)
    #[arg(long, value_enum)]
    path_style: Option<PathStyle>,
    /// 검색 통계를 stderr에 출력
    #[arg(long)]
    stats: bool,
}

impl Args {
    fn before(&self) -> usize {
        self.before_context.or(self.context).unwrap_or(0)
    }
    fn after(&self) -> usize {
        self.after_context.or(self.context).unwrap_or(0)
    }
}

struct DiagnosticBudget {
    emitted: u64,
    remaining: u64,
}

struct Shared {
    output: Mutex<BufWriter<io::Stdout>>,
    stop: AtomicBool,
    broken_pipe: AtomicBool,
    error_count: AtomicU64,
    output_failed: AtomicBool,
    stop_reason: AtomicU8,
    found: AtomicBool,
    records: AtomicU64,
    files: AtomicU64,
    bytes: AtomicU64,
    matched_lines: AtomicU64,
    limit: u64,
    json: bool,
    output_budget: Option<Mutex<u64>>,
    diagnostic_budget: Mutex<DiagnosticBudget>,
    max_errors: u64,
    started: Instant,
    timeout: Option<Duration>,
    incomplete: AtomicBool,
    contexts: AtomicU64,
    searched: AtomicU64,
    binaries: AtomicU64,
    oversized: AtomicU64,
    skipped_lines: AtomicU64,
    cwd: PathBuf,
    path_style: PathStyle,
}

// This exceeds the maximum serialized Summary size, including its newline.
const SUMMARY_RESERVE: u64 = 768;
const RESULT_LIMIT: u8 = 1;
const OUTPUT_LIMIT: u8 = 2;
const OUTPUT_ERROR: u8 = 3;
const TIME_LIMIT: u8 = 4;

impl Shared {
    fn new(json: bool, limit: Option<u64>, output_limit: Option<u64>) -> Self {
        Self {
            output: Mutex::new(BufWriter::with_capacity(64 * 1024, io::stdout())),
            stop: AtomicBool::new(false),
            broken_pipe: AtomicBool::new(false),
            error_count: AtomicU64::new(0),
            output_failed: AtomicBool::new(false),
            stop_reason: AtomicU8::new(0),
            found: AtomicBool::new(false),
            records: AtomicU64::new(0),
            files: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            matched_lines: AtomicU64::new(0),
            limit: limit.unwrap_or(u64::MAX),
            json,
            output_budget: output_limit.map(|n| Mutex::new(n - SUMMARY_RESERVE)),
            diagnostic_budget: Mutex::new(DiagnosticBudget {
                emitted: 0,
                remaining: 16384,
            }),
            max_errors: 20,
            started: Instant::now(),
            timeout: None,
            incomplete: AtomicBool::new(false),
            contexts: AtomicU64::new(0),
            searched: AtomicU64::new(0),
            binaries: AtomicU64::new(0),
            oversized: AtomicU64::new(0),
            skipped_lines: AtomicU64::new(0),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            path_style: if json {
                PathStyle::Relative
            } else {
                PathStyle::Native
            },
        }
    }

    fn stopped(&self) -> bool {
        if !self.stop.load(Ordering::Relaxed)
            && self
                .timeout
                .is_some_and(|timeout| self.started.elapsed() >= timeout)
        {
            self.stop_for(TIME_LIMIT);
        }
        self.stop.load(Ordering::Relaxed)
    }

    fn stop_for(&self, reason: u8) {
        let _ = self
            .stop_reason
            .compare_exchange(0, reason, Ordering::Relaxed, Ordering::Relaxed);
        self.stop.store(true, Ordering::Relaxed);
    }

    fn reserve(&self, bytes: usize, is_result: bool) -> bool {
        if self.stopped() {
            return false;
        }
        // Serialize byte reservations across workers; normal output keeps the atomic fast path.
        let mut remaining = self
            .output_budget
            .as_ref()
            .map(|budget| budget.lock().unwrap());
        if self.stopped() {
            return false;
        }
        if let Some(remaining) = remaining.as_ref()
            && bytes as u64 > **remaining
        {
            self.stop_for(OUTPUT_LIMIT);
            return false;
        }
        if !self.reserve_record(is_result) {
            return false;
        }
        if let Some(remaining) = remaining.as_mut() {
            **remaining -= bytes as u64;
        }
        true
    }

    fn reserve_record(&self, is_result: bool) -> bool {
        if self.stopped() {
            return false;
        }
        if !is_result {
            self.contexts.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        let mut n = self.records.load(Ordering::Relaxed);
        loop {
            if n >= self.limit {
                return false;
            }
            match self
                .records
                .compare_exchange_weak(n, n + 1, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    self.found.store(true, Ordering::Relaxed);
                    if n + 1 == self.limit {
                        self.stop_for(RESULT_LIMIT);
                    }
                    return true;
                }
                Err(current) => n = current,
            }
        }
    }

    fn error(&self, mut diagnostic: Diagnostic) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
        diagnostic.format_path(&self.cwd, self.path_style);
        let mut budget = self.diagnostic_budget.lock().unwrap();
        if budget.emitted < self.max_errors
            && let Some(bytes) = diagnostic.encode(self.json, budget.remaining)
        {
            budget.remaining -= bytes.len() as u64;
            budget.emitted += 1;
            let _ = io::stderr().lock().write_all(&bytes);
        }
    }

    fn output_error(&self, error: io::Error) {
        self.stop_for(OUTPUT_ERROR);
        if self.output_failed.swap(true, Ordering::Relaxed) {
            return;
        }
        if error.kind() == io::ErrorKind::BrokenPipe {
            self.broken_pipe.store(true, Ordering::Relaxed);
        } else {
            self.error(Diagnostic::io("output_error", "output", None, &error));
        }
    }

    fn summary(&self) -> Summary {
        let errors = self.error_count.load(Ordering::Relaxed);
        let reason = self.stop_reason.load(Ordering::Relaxed);
        let emitted = self.diagnostic_budget.lock().unwrap().emitted;
        let incomplete = self.incomplete.load(Ordering::Relaxed);
        Summary {
            r#type: "summary",
            schema_version: 1,
            records_emitted: self.records.load(Ordering::Relaxed)
                + self.contexts.load(Ordering::Relaxed),
            matched: self.found.load(Ordering::Relaxed),
            scan_complete: !self.stop.load(Ordering::Relaxed) && errors == 0 && !incomplete,
            stop_reason: match reason {
                RESULT_LIMIT => "result_limit",
                OUTPUT_LIMIT => "output_limit",
                OUTPUT_ERROR => "output_error",
                TIME_LIMIT => "time_limit",
                _ if errors > 0 => "error",
                _ if incomplete => "resource_limit",
                _ => "completed",
            },
            error_count: errors,
            errors_emitted: emitted,
            errors_suppressed: errors - emitted,
            context_records_emitted: self.contexts.load(Ordering::Relaxed),
            files_visited: self.files.load(Ordering::Relaxed),
            files_searched: self.searched.load(Ordering::Relaxed),
            binary_files_skipped: self.binaries.load(Ordering::Relaxed),
            oversized_files_skipped: self.oversized.load(Ordering::Relaxed),
            lines_skipped: self.skipped_lines.load(Ordering::Relaxed),
        }
    }

    fn finish(&self) -> u8 {
        if self.json && !self.output_failed.load(Ordering::Relaxed) {
            let mut bytes = serde_json::to_vec(&self.summary()).unwrap();
            bytes.push(b'\n');
            assert!(bytes.len() as u64 <= SUMMARY_RESERVE);
            let result = self.output.lock().unwrap().write_all(&bytes);
            if let Err(error) = result {
                self.output_error(error);
            }
        }
        let result = self.output.lock().unwrap().flush();
        if let Err(error) = result {
            self.output_error(error);
        }
        if self.error_count.load(Ordering::Relaxed) > 0
            || self.incomplete.load(Ordering::Relaxed)
            || self.stop_reason.load(Ordering::Relaxed) == TIME_LIMIT
        {
            2
        } else if self.broken_pipe.load(Ordering::Relaxed) || self.found.load(Ordering::Relaxed) {
            0
        } else {
            1
        }
    }
}

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
];

fn search(args: &mut Args, shared: &Shared) -> Result<(), Diagnostic> {
    let matcher = if args.files {
        if let Some(path) = args.pattern.take() {
            args.paths.insert(0, PathBuf::from(path));
        }
        None
    } else if !args.expressions.is_empty() {
        if let Some(path) = args.pattern.take() {
            args.paths.insert(0, PathBuf::from(path));
        }
        Some(
            Matcher::new(&args.expressions, args)
                .map_err(|m| Diagnostic::new("invalid_pattern", "pattern", m))?,
        )
    } else {
        let pattern = args.pattern.as_ref().unwrap().to_str().ok_or_else(|| {
            Diagnostic::new("invalid_pattern", "pattern", "PATTERN must be valid UTF-8")
        })?;
        Some(
            Matcher::new(&[pattern.to_owned()], args)
                .map_err(|message| Diagnostic::new("invalid_pattern", "pattern", message))?,
        )
    };
    if args.paths.is_empty() {
        args.paths.push(PathBuf::from("."));
    }

    let mut walker = WalkBuilder::new(&args.paths[0]);
    for path in &args.paths[1..] {
        walker.add(path);
    }
    let use_ignores = !(args.no_ignore || args.all);
    let threads = args.threads.map(usize::from).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(12)
    });
    walker
        .standard_filters(use_ignores)
        .hidden(!(args.hidden || args.all))
        .follow_links(args.follow)
        .threads(threads)
        .skip_stdout(true);
    if use_ignores {
        walker.add_custom_ignore_filename(".qfindignore");
        walker.filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|t| t.is_dir())
                || !SKIP_DIRS.iter().any(|name| entry.file_name() == *name)
        });
    }
    let mut overrides = OverrideBuilder::new(
        std::env::current_dir()
            .map_err(|e| Diagnostic::io("current_directory_error", "setup", None, &e))?,
    );
    for glob in &args.globs {
        overrides
            .add(glob)
            .map_err(|e| Diagnostic::new("invalid_glob", "glob", e.to_string()))?;
    }
    walker.overrides(
        overrides
            .build()
            .map_err(|e| Diagnostic::new("invalid_glob", "glob", e.to_string()))?,
    );

    if args.sort.is_some() {
        let mut paths = Vec::new();
        for entry in walker.build() {
            if shared.stopped() {
                break;
            }
            match entry {
                Ok(entry) => {
                    if let Some(error) = entry.error() {
                        shared.error(Diagnostic::traversal(error, Some(entry.path())));
                    }
                    if entry.file_type().is_some_and(|t| t.is_file()) {
                        paths.push(entry.into_path());
                    }
                }
                Err(error) => shared.error(Diagnostic::traversal(&error, None)),
            }
        }
        paths.sort_by_cached_key(|path| paths::display(path, &shared.cwd, PathStyle::Relative));
        paths.dedup();
        let mut worker = Worker::new(args, matcher.as_ref(), shared);
        for path in paths {
            if worker.stopped() {
                break;
            }
            worker.visit(&path);
        }
    } else {
        walker.build_parallel().run(|| {
            let mut worker = Worker::new(args, matcher.as_ref(), shared);
            Box::new(move |entry| {
                if worker.stopped() {
                    return WalkState::Quit;
                }
                match entry {
                    Ok(entry) => {
                        if let Some(error) = entry.error() {
                            worker.report_error(error, Some(entry.path()));
                        }
                        if entry.file_type().is_some_and(|t| t.is_file()) {
                            worker.visit(entry.path());
                        }
                    }
                    Err(error) => worker.report_error(&error, None),
                }
                if worker.stopped() {
                    WalkState::Quit
                } else {
                    WalkState::Continue
                }
            })
        });
    }
    Ok(())
}

fn run(mut args: Args) -> u8 {
    let start = Instant::now();
    let mut shared = Shared::new(args.json, args.max_results, args.max_output_bytes);
    shared.max_errors = args.max_errors;
    shared.diagnostic_budget.get_mut().unwrap().remaining = args.max_error_bytes;
    shared.timeout = args.timeout_ms.map(Duration::from_millis);
    if let Some(style) = args.path_style {
        shared.path_style = style;
    }
    if let Err(error) = search(&mut args, &shared) {
        shared.error(error);
    }
    let code = shared.finish();
    if args.stats {
        let files = shared.files.load(Ordering::Relaxed);
        let bytes = shared.bytes.load(Ordering::Relaxed);
        let matched_lines = shared.matched_lines.load(Ordering::Relaxed);
        let results = shared.records.load(Ordering::Relaxed);
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let coverage = shared.summary();
        if args.json {
            let mut stats = serde_json::to_value(&coverage).unwrap();
            stats["type"] = "stats".into();
            stats["files"] = files.into();
            stats["bytes"] = bytes.into();
            stats["matched_lines"] = matched_lines.into();
            stats["results"] = results.into();
            stats["elapsed_ms"] = elapsed_ms.into();
            let _ = writeln!(io::stderr().lock(), "{stats}");
        } else {
            let _ = writeln!(
                io::stderr().lock(),
                "files={files} bytes={bytes} matched_lines={matched_lines} results={results} elapsed_ms={elapsed_ms:.3} files_searched={} binary_files_skipped={} oversized_files_skipped={} lines_skipped={} errors_suppressed={}",
                coverage.files_searched,
                coverage.binary_files_skipped,
                coverage.oversized_files_skipped,
                coverage.lines_skipped,
                coverage.errors_suppressed
            );
        }
    }
    code
}

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    match Args::try_parse_from(&argv) {
        Ok(args) => ExitCode::from(run(args)),
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        Err(error) => {
            let shared = Shared::new(json_requested(&argv), None, None);
            shared.error(Diagnostic::new(
                "invalid_arguments",
                "arguments",
                error.to_string(),
            ));
            ExitCode::from(shared.finish())
        }
    }
}

fn json_requested(argv: &[OsString]) -> bool {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg == "--json" {
            return true;
        }
        if [
            "-g",
            "--glob",
            "-j",
            "--threads",
            "--max-results",
            "--max-output-bytes",
            "--max-line-bytes",
            "-e",
            "--expression",
            "-A",
            "--after-context",
            "-B",
            "--before-context",
            "-C",
            "--context",
            "--timeout-ms",
            "--max-file-bytes",
            "--max-line-buffer-bytes",
            "--max-errors",
            "--max-error-bytes",
            "--sort",
            "--path-style",
        ]
        .iter()
        .any(|name| arg == *name)
        {
            args.next();
        }
    }
    false
}
