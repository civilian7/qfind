use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;

use crate::{Args, PathStyle, Shared, paths};

#[derive(Serialize)]
pub(crate) struct Summary {
    pub r#type: &'static str,
    pub schema_version: u8,
    pub records_emitted: u64,
    pub matched: bool,
    pub scan_complete: bool,
    pub stop_reason: &'static str,
    pub error_count: u64,
    pub errors_emitted: u64,
    pub errors_suppressed: u64,
    pub context_records_emitted: u64,
    pub files_visited: u64,
    pub files_searched: u64,
    pub binary_files_skipped: u64,
    pub oversized_files_skipped: u64,
    pub lines_skipped: u64,
}

#[derive(Serialize)]
pub(crate) struct Diagnostic {
    r#type: &'static str,
    code: &'static str,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    os_error: Option<i32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    message_truncated: bool,
}

impl Diagnostic {
    pub fn new(code: &'static str, stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            r#type: "error",
            code,
            stage,
            path: None,
            message: message.into(),
            os_error: None,
            message_truncated: false,
        }
    }

    pub fn io(
        code: &'static str,
        stage: &'static str,
        path: Option<&Path>,
        error: &io::Error,
    ) -> Self {
        let mut diagnostic = Self::new(code, stage, error.to_string());
        diagnostic.path = path.map(|p| p.to_string_lossy().into_owned());
        diagnostic.os_error = error.raw_os_error();
        diagnostic
    }

    pub fn traversal(error: &ignore::Error, path: Option<&Path>) -> Self {
        let mut diagnostic = Self::new("traversal_error", "traversal", error.to_string());
        diagnostic.path = path
            .or_else(|| traversal_path(error))
            .map(|p| p.to_string_lossy().into_owned());
        diagnostic.os_error = error.io_error().and_then(io::Error::raw_os_error);
        diagnostic
    }

    pub fn format_path(&mut self, cwd: &Path, style: PathStyle) {
        if let Some(path) = &mut self.path {
            *path = paths::display(Path::new(path), cwd, style);
        }
    }

    pub fn encode(mut self, json: bool, limit: u64) -> Option<Vec<u8>> {
        loop {
            let bytes = if json {
                let mut bytes = serde_json::to_vec(&self).unwrap();
                bytes.push(b'\n');
                bytes
            } else if let Some(path) = &self.path {
                format!("qfind: {path}: {}\n", self.message).into_bytes()
            } else {
                format!("qfind: {}\n", self.message).into_bytes()
            };
            if bytes.len() as u64 <= limit {
                return Some(bytes);
            }
            if self.message.is_empty() && self.path.as_ref().is_none_or(|p| p.is_empty()) {
                return None;
            }
            self.message_truncated = true;
            let end = utf8_floor(&self.message, self.message.len() / 2);
            self.message.truncate(end);
            if let Some(path) = &mut self.path {
                path.truncate(utf8_floor(path, path.len() / 2));
            }
        }
    }
}

fn traversal_path(error: &ignore::Error) -> Option<&Path> {
    match error {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            traversal_path(err)
        }
        _ => None,
    }
}

#[derive(Serialize)]
pub(crate) struct Record<'a> {
    pub r#type: &'static str,
    pub path: std::borrow::Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<std::borrow::Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_start_byte: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_original_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_end: Option<u64>,
}

impl<'a> Record<'a> {
    fn file(path: &'a Path, count: Option<u64>) -> Self {
        Self {
            r#type: if count.is_some() { "count" } else { "file" },
            path: path.to_string_lossy(),
            line: None,
            column: None,
            text: None,
            text_base64: None,
            count,
            text_truncated: None,
            text_start_byte: None,
            text_original_bytes: None,
            end_column: None,
            byte_start: None,
            byte_end: None,
        }
    }
}

pub(crate) struct Output<'a> {
    shared: &'a Shared,
    json: bool,
    max_line_bytes: usize,
    path_style: PathStyle,
    buffer: Vec<u8>,
    encoded: Vec<u8>,
    // Last displayed path; consecutive results usually come from the same file.
    cached_path: std::path::PathBuf,
    cached_display: String,
    // `cached_display` as a quoted JSON string, ready to splice into records.
    cached_json: Vec<u8>,
}

impl<'a> Output<'a> {
    fn cache_display(&mut self, path: &Path) {
        if self.cached_display.is_empty() || self.cached_path.as_os_str() != path.as_os_str() {
            self.cached_display = paths::display(path, &self.shared.cwd, self.path_style);
            self.cached_path.clear();
            self.cached_path.push(path);
            self.cached_json.clear();
            serde_json::to_writer(&mut self.cached_json, &self.cached_display).unwrap();
        }
    }

    pub fn new(shared: &'a Shared, args: &Args) -> Self {
        Self {
            shared,
            json: args.json,
            path_style: args.path_style.unwrap_or(if args.json {
                PathStyle::Relative
            } else {
                PathStyle::Native
            }),
            max_line_bytes: match args.max_line_bytes.unwrap_or(4096) {
                0 => usize::MAX,
                n => usize::try_from(n).unwrap_or(usize::MAX),
            },
            buffer: Vec::with_capacity(64 * 1024),
            encoded: Vec::with_capacity(512),
            cached_path: std::path::PathBuf::new(),
            cached_display: String::new(),
            cached_json: Vec::new(),
        }
    }

    pub fn file(&mut self, path: &Path, count: Option<u64>) {
        if self.shared.stopped() {
            return;
        }
        self.shared
            .found
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let start = self.buffer.len();
        self.cache_display(path);
        let display = &self.cached_display;
        if self.json {
            let mut record = Record::file(path, count);
            record.path = display.as_str().into();
            serde_json::to_writer(&mut self.buffer, &record).unwrap();
            self.buffer.push(b'\n');
        } else if let Some(count) = count {
            writeln!(self.buffer, "{display}:{count}").unwrap();
        } else {
            writeln!(self.buffer, "{display}").unwrap();
        }
        self.commit(start, true);
    }

    pub fn line(
        &mut self,
        path: &Path,
        line: u64,
        offset: u64,
        range: Option<Range<usize>>,
        text: &[u8],
    ) {
        if self.shared.stopped() {
            return;
        }
        let start = self.buffer.len();
        self.cache_display(path);
        let display = &self.cached_display;
        let is_result = range.is_some();
        if self.json {
            let original_len = text.len();
            let valid = std::str::from_utf8(text).ok();
            // Keep the budget locked through adaptive encoding and reservation.
            let mut budget = self
                .shared
                .output_budget
                .as_ref()
                .map(|b| b.lock().unwrap());
            let mut limit = self.max_line_bytes.min(text.len());
            if let Some(remaining) = budget.as_ref() {
                limit = limit.min(usize::try_from(**remaining).unwrap_or(usize::MAX));
            }
            let encoded = loop {
                let (text, excerpt_start) =
                    excerpt(text, range.as_ref().map_or(0, |r| r.start), limit, valid);
                let truncated = text.len() < original_len;
                if !truncated && valid.is_some() {
                    // Same bytes as `Record`, minus the optional-field machinery.
                    let e = &mut self.encoded;
                    e.clear();
                    e.extend_from_slice(if is_result {
                        br#"{"type":"match","path":"#
                    } else {
                        br#"{"type":"context","path":"#
                    });
                    e.extend_from_slice(&self.cached_json);
                    e.extend_from_slice(br#","line":"#);
                    push_decimal(e, line);
                    if let Some(r) = range.as_ref() {
                        e.extend_from_slice(br#","column":"#);
                        push_decimal(e, r.start as u64 + 1);
                    }
                    e.extend_from_slice(br#","text":"#);
                    serde_json::to_writer(&mut *e, valid.unwrap()).unwrap();
                    if let Some(r) = range.as_ref() {
                        e.extend_from_slice(br#","end_column":"#);
                        push_decimal(e, r.end as u64 + 1);
                    }
                    e.extend_from_slice(br#","byte_start":"#);
                    push_decimal(e, offset + range.as_ref().map_or(0, |r| r.start as u64));
                    e.extend_from_slice(br#","byte_end":"#);
                    push_decimal(
                        e,
                        offset + range.as_ref().map_or(original_len as u64, |r| r.end as u64),
                    );
                    e.extend_from_slice(b"}
");
                    if budget.as_ref().is_none_or(|b| e.len() as u64 <= **b) {
                        break true;
                    }
                    if limit == 0 {
                        break false;
                    }
                    limit /= 2;
                    continue;
                }
                let record = Record {
                    r#type: if is_result { "match" } else { "context" },
                    path: display.as_str().into(),
                    line: Some(line),
                    column: range.as_ref().map(|r| r.start + 1),
                    text: Some(String::from_utf8_lossy(text)),
                    text_base64: std::str::from_utf8(text)
                        .is_err()
                        .then(|| STANDARD.encode(text)),
                    count: None,
                    text_truncated: truncated.then_some(true),
                    text_start_byte: truncated.then_some(excerpt_start),
                    text_original_bytes: truncated.then_some(original_len),
                    end_column: range.as_ref().map(|r| r.end + 1),
                    byte_start: Some(offset + range.as_ref().map_or(0, |r| r.start as u64)),
                    byte_end: Some(
                        offset + range.as_ref().map_or(original_len as u64, |r| r.end as u64),
                    ),
                };
                self.encoded.clear();
                serde_json::to_writer(&mut self.encoded, &record).unwrap();
                self.encoded.push(b'\n');
                if budget
                    .as_ref()
                    .is_none_or(|b| self.encoded.len() as u64 <= **b)
                {
                    break true;
                }
                if limit == 0 {
                    break false;
                }
                limit /= 2;
            };
            if encoded {
                if self.shared.reserve_record(is_result) {
                    if let Some(budget) = budget.as_mut() {
                        **budget -= self.encoded.len() as u64;
                    }
                    self.buffer.extend_from_slice(&self.encoded);
                }
            } else {
                self.shared.stop_for(crate::OUTPUT_LIMIT);
            }
            drop(budget);
            if self.buffer.len() >= 64 * 1024 {
                self.flush();
            }
        } else {
            self.buffer.extend_from_slice(display.as_bytes());
            if let Some(range) = range {
                self.buffer.push(b':');
                push_decimal(&mut self.buffer, line);
                self.buffer.push(b':');
                push_decimal(&mut self.buffer, range.start as u64 + 1);
                self.buffer.push(b':');
            } else {
                self.buffer.push(b'-');
                push_decimal(&mut self.buffer, line);
                self.buffer.push(b'-');
            }
            self.buffer.extend_from_slice(text);
            self.buffer.push(b'\n');
            self.commit(start, is_result);
        }
    }

    fn commit(&mut self, start: usize, is_result: bool) {
        if !self.shared.reserve(self.buffer.len() - start, is_result) {
            self.buffer.truncate(start);
            return;
        }
        if self.buffer.len() >= 64 * 1024 {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        if !self
            .shared
            .output_failed
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let result = self.shared.output.lock().unwrap().write_all(&self.buffer);
            if let Err(error) = result {
                self.shared.output_error(error);
            }
        }
        self.buffer.clear();
    }
}

fn push_decimal(buffer: &mut Vec<u8>, mut value: u64) {
    let mut digits = [0_u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    buffer.extend_from_slice(&digits[at..]);
}

fn utf8_floor(text: &str, mut end: usize) -> usize {
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn excerpt<'a>(
    text: &'a [u8],
    position: usize,
    limit: usize,
    valid: Option<&str>,
) -> (&'a [u8], usize) {
    if text.len() <= limit {
        return (text, 0);
    }
    let mut start = position.saturating_sub(limit / 2).min(text.len() - limit);
    let mut end = start + limit;
    if let Some(valid) = valid {
        while !valid.is_char_boundary(start) {
            start += 1;
        }
        while !valid.is_char_boundary(end) {
            end -= 1;
        }
        end = end.max(start);
    }
    (&text[start..end], start)
}

impl Drop for Output<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::Summary;

    #[test]
    fn summary_fits_reserved_space_even_with_maximum_counters() {
        let summary = Summary {
            r#type: "summary",
            schema_version: 1,
            records_emitted: u64::MAX,
            matched: false,
            scan_complete: false,
            stop_reason: "resource_limit",
            error_count: u64::MAX,
            errors_emitted: u64::MAX,
            errors_suppressed: u64::MAX,
            context_records_emitted: u64::MAX,
            files_visited: u64::MAX,
            files_searched: u64::MAX,
            binary_files_skipped: u64::MAX,
            oversized_files_skipped: u64::MAX,
            lines_skipped: u64::MAX,
        };
        assert!((serde_json::to_vec(&summary).unwrap().len() as u64) < crate::SUMMARY_RESERVE);
    }
}
