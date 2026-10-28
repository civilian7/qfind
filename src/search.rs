use memchr::{memchr, memchr_iter, memmem, memrchr};
use regex::bytes::{Regex, RegexBuilder};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::{Args, Diagnostic, Output, Shared};

const READ_SIZE: usize = 256 * 1024;
const BINARY_SAMPLE: usize = 8 * 1024;

#[derive(Clone)]
pub(crate) enum Matcher {
    Literal(Box<memmem::Finder<'static>>),
    Regex { regexes: Vec<Regex>, word: bool },
}

impl Matcher {
    pub fn new(patterns: &[String], args: &Args) -> Result<Self, String> {
        if patterns.iter().any(|pattern| pattern.contains('\n')) {
            return Err(
                "PATTERN cannot contain a newline; searches operate on individual lines".into(),
            );
        }
        if patterns.len() != 1
            || args.regex
            || args.ignore_case
            || args.word_regexp
            || args.line_regexp
        {
            // Compile regex inputs separately so named groups and flags stay local to each -e.
            let expressions = if args.regex {
                patterns.to_vec()
            } else {
                vec![
                    patterns
                        .iter()
                        .map(|p| format!("(?:{})", regex::escape(p)))
                        .collect::<Vec<_>>()
                        .join("|"),
                ]
            };
            let regexes = expressions
                .into_iter()
                .map(|expression| {
                    let expression = if args.line_regexp {
                        format!("^(?:{expression})$")
                    } else if args.word_regexp {
                        format!(r"(?:^|\W)({expression})(?:$|\W)")
                    } else {
                        expression
                    };
                    RegexBuilder::new(&expression)
                        .case_insensitive(args.ignore_case)
                        .build()
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Self::Regex {
                regexes,
                word: args.word_regexp && !args.line_regexp,
            })
        } else {
            Ok(Self::Literal(Box::new(
                memmem::Finder::new(&patterns[0]).into_owned(),
            )))
        }
    }

    fn find(&self, text: &[u8]) -> Option<Range<usize>> {
        match self {
            Self::Literal(finder) => finder
                .find(text)
                .map(|start| start..start + finder.needle().len()),
            Self::Regex { regexes, word } if regexes.len() == 1 => {
                let regex = &regexes[0];
                if *word {
                    regex.captures(text)?.get(1).map(|m| m.range())
                } else {
                    regex.find(text).map(|m| m.range())
                }
            }
            Self::Regex { regexes, word } => regexes
                .iter()
                .filter_map(|regex| {
                    if *word {
                        regex.captures(text)?.get(1).map(|m| m.range())
                    } else {
                        regex.find(text).map(|m| m.range())
                    }
                })
                .min_by_key(|range| range.start),
        }
    }
}

#[derive(Default)]
struct Lines {
    number: u64,
    offset: u64,
    count: u64,
    before: VecDeque<(u64, u64, Vec<u8>)>,
    after_until: u64,
    last_emitted: u64,
    skipped: bool,
}

pub(crate) struct Worker<'a> {
    args: &'a Args,
    matcher: Option<Matcher>,
    shared: &'a Shared,
    output: Output<'a>,
    buffer: Vec<u8>,
}

impl<'a> Worker<'a> {
    pub fn new(args: &'a Args, matcher: Option<&Matcher>, shared: &'a Shared) -> Self {
        // Regex clones share compiled code but maintain separate search caches per worker.
        let matcher = matcher.cloned();
        Self {
            args,
            matcher,
            shared,
            output: Output::new(shared, args),
            buffer: Vec::new(),
        }
    }

    pub fn stopped(&self) -> bool {
        self.shared.stopped()
    }

    pub fn report_error(&self, error: &ignore::Error, path: Option<&Path>) {
        self.shared.error(Diagnostic::traversal(error, path));
    }

    pub fn visit(&mut self, path: &Path) {
        self.shared.files.fetch_add(1, Ordering::Relaxed);
        if self.args.files {
            self.output.file(path, None);
            return;
        }
        if let Err(error) = self.search(path) {
            self.shared
                .error(Diagnostic::io("read_error", "read", Some(path), &error));
        }
    }

    fn search(&mut self, path: &Path) -> io::Result<()> {
        let mut file = File::open(path)?;
        if self
            .args
            .max_file_bytes
            .is_some_and(|max| file.metadata().is_ok_and(|m| m.len() > max))
        {
            self.oversized();
            return Ok(());
        }
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.resize(READ_SIZE, 0);
        let result = if self.args.before() > 0
            || self.args.after() > 0
            || self.args.max_line_buffer_bytes.is_some()
        {
            self.read_lines(path, &mut file, &mut buffer)
        } else {
            self.read_file(path, &mut file, &mut buffer)
        };
        self.buffer = buffer;
        result
    }

    fn read_file(&mut self, path: &Path, file: &mut File, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut used = 0;
        let mut first_read = true;
        let mut line_number = 1;
        let mut count = 0;
        let mut total = 0;
        let mut offset = 0;
        loop {
            if self.stopped() {
                break;
            }
            if used == buffer.len() {
                let size = buffer
                    .len()
                    .checked_mul(2)
                    .ok_or_else(|| io::Error::other("line exceeds addressable memory"))?;
                buffer.resize(size, 0);
            }
            let Some(read) =
                self.read_chunk(file, &mut buffer[used..], &mut total, &mut first_read)?
            else {
                return Ok(());
            };
            used += read;
            let end = if read == 0 {
                used
            } else {
                memrchr(b'\n', &buffer[..used]).map_or(0, |n| n + 1)
            };
            if end > 0 {
                let keep_reading =
                    self.search_block(path, &buffer[..end], &mut line_number, offset, &mut count);
                if !keep_reading {
                    break;
                }
                buffer.copy_within(end..used, 0);
                used -= end;
                offset += end as u64;
            }
            if read == 0 {
                break;
            }
        }
        if count > 0 {
            if self.args.files_with_matches {
                self.output.file(path, None);
            } else if self.args.count {
                self.output.file(path, Some(count));
            }
        }
        Ok(())
    }

    fn oversized(&self) {
        self.shared.oversized.fetch_add(1, Ordering::Relaxed);
        self.shared.incomplete.store(true, Ordering::Relaxed);
    }

    fn read_chunk(
        &self,
        file: &mut File,
        buffer: &mut [u8],
        total: &mut u64,
        first: &mut bool,
    ) -> io::Result<Option<usize>> {
        let cap = self.args.max_file_bytes.map_or(buffer.len(), |max| {
            usize::try_from(max.saturating_sub(*total).saturating_add(1))
                .unwrap_or(usize::MAX)
                .min(buffer.len())
        });
        let read = loop {
            match file.read(&mut buffer[..cap]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    if self.stopped() {
                        return Ok(None);
                    }
                }
                other => break other?,
            }
        };
        *total += read as u64;
        self.shared.bytes.fetch_add(read as u64, Ordering::Relaxed);
        if self.args.max_file_bytes.is_some_and(|max| *total > max) {
            self.oversized();
            return Ok(None);
        }
        if *first {
            *first = false;
            if !self.args.text && memchr(0, &buffer[..read.min(BINARY_SAMPLE)]).is_some() {
                self.shared.binaries.fetch_add(1, Ordering::Relaxed);
                return Ok(None);
            }
            self.shared.searched.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Some(read))
    }

    fn finish_lines(&mut self, path: &Path, lines: &Lines) {
        if lines.count > 0 {
            if self.args.files_with_matches {
                self.output.file(path, None);
            } else if self.args.count && !lines.skipped {
                self.output.file(path, Some(lines.count));
            }
        }
    }

    fn skip_line(&self, lines: &mut Lines) {
        self.shared.skipped_lines.fetch_add(1, Ordering::Relaxed);
        self.shared.incomplete.store(true, Ordering::Relaxed);
        lines.skipped = true;
        lines.before.clear();
        lines.after_until = 0;
    }

    fn read_lines(&mut self, path: &Path, file: &mut File, buffer: &mut [u8]) -> io::Result<()> {
        let mut lines = Lines {
            number: 1,
            ..Lines::default()
        };
        let mut pending = Vec::new();
        let mut discarding = false;
        let mut line_bytes = 0_u64;
        let mut first = true;
        let mut total = 0;
        loop {
            if self.stopped() {
                break;
            }
            let Some(read) = self.read_chunk(file, buffer, &mut total, &mut first)? else {
                return Ok(());
            };
            if read == 0 {
                if line_bytes > 0 && !discarding {
                    if self
                        .args
                        .max_line_buffer_bytes
                        .is_some_and(|max| pending.len() as u64 > max)
                    {
                        self.skip_line(&mut lines);
                    } else {
                        self.process_line(path, &pending, &mut lines);
                    }
                }
                break;
            }
            let mut cursor = 0;
            while cursor < read {
                if self.stopped() {
                    break;
                }
                let end = memchr(b'\n', &buffer[cursor..read]).map(|n| cursor + n);
                let part = &buffer[cursor..end.unwrap_or(read)];
                let raw_len = pending.len().saturating_add(part.len());
                let cr = end.is_some() && part.last().or_else(|| pending.last()) == Some(&b'\r');
                let too_long = self.args.max_line_buffer_bytes.is_some_and(|max| {
                    if end.is_some() {
                        raw_len.saturating_sub(usize::from(cr)) as u64 > max
                    } else {
                        raw_len as u64 > max.saturating_add(1)
                    }
                });
                line_bytes += part.len() as u64;
                if !discarding && too_long {
                    self.skip_line(&mut lines);
                    pending.clear();
                    discarding = true;
                }
                if let Some(end) = end {
                    if !discarding {
                        let keep = if pending.is_empty() {
                            self.process_line(path, strip_cr(part, true), &mut lines)
                        } else {
                            pending.extend_from_slice(part);
                            self.process_line(path, strip_cr(&pending, true), &mut lines)
                        };
                        if !keep {
                            self.finish_lines(path, &lines);
                            return Ok(());
                        }
                    }
                    lines.offset += line_bytes + 1;
                    lines.number += 1;
                    line_bytes = 0;
                    discarding = false;
                    pending.clear();
                    cursor = end + 1;
                } else {
                    if !discarding {
                        pending.extend_from_slice(part);
                    }
                    break;
                }
            }
        }
        self.finish_lines(path, &lines);
        Ok(())
    }

    fn process_line(&mut self, path: &Path, text: &[u8], lines: &mut Lines) -> bool {
        if let Some(range) = self.matcher.as_ref().unwrap().find(text) {
            lines.count += 1;
            self.shared.matched_lines.fetch_add(1, Ordering::Relaxed);
            self.shared.found.store(true, Ordering::Relaxed);
            if self.args.quiet {
                self.shared.stop.store(true, Ordering::Relaxed);
                return false;
            }
            if self.args.files_with_matches {
                return false;
            }
            if !self.args.count {
                for (number, offset, text) in &lines.before {
                    if *number > lines.last_emitted {
                        self.output.line(path, *number, *offset, None, text);
                        lines.last_emitted = *number;
                    }
                }
                self.output
                    .line(path, lines.number, lines.offset, Some(range), text);
                lines.last_emitted = lines.number;
                lines.after_until = lines.number.saturating_add(self.args.after() as u64);
            }
        } else if lines.number <= lines.after_until && lines.number > lines.last_emitted {
            self.output
                .line(path, lines.number, lines.offset, None, text);
            lines.last_emitted = lines.number;
        }
        if self.args.before() > 0 {
            if lines.before.len() == self.args.before() {
                lines.before.pop_front();
            }
            lines
                .before
                .push_back((lines.number, lines.offset, text.to_vec()));
        }
        !self.stopped()
    }

    fn matched(
        &mut self,
        path: &Path,
        line: u64,
        offset: u64,
        range: Range<usize>,
        text: &[u8],
        count: &mut u64,
    ) -> bool {
        *count += 1;
        self.shared.matched_lines.fetch_add(1, Ordering::Relaxed);
        self.shared.found.store(true, Ordering::Relaxed);
        if self.args.quiet {
            self.shared.stop.store(true, Ordering::Relaxed);
            return false;
        }
        if self.args.files_with_matches {
            return false;
        }
        if !self.args.count {
            self.output.line(path, line, offset, Some(range), text);
        }
        !self.stopped()
    }

    fn search_block(
        &mut self,
        path: &Path,
        data: &[u8],
        line: &mut u64,
        offset: u64,
        count: &mut u64,
    ) -> bool {
        if let Some(Matcher::Literal(finder)) = self.matcher.as_ref() {
            let needle_len = finder.needle().len();
            // A block-wide match is safe only when it cannot include CRLF's CR.
            if (self.args.quiet || self.args.files_with_matches)
                && !finder.needle().contains(&b'\r')
            {
                return if finder.find(data).is_some() {
                    self.matched(path, 0, 0, 0..0, &[], count)
                } else {
                    true
                };
            }
            let mut cursor = 0;
            while cursor < data.len() {
                if self.stopped() {
                    return false;
                }
                let Some(Matcher::Literal(finder)) = self.matcher.as_ref() else {
                    unreachable!()
                };
                let Some(found) = finder.find(&data[cursor..]) else {
                    break;
                };
                let position = cursor + found;
                *line += memchr_iter(b'\n', &data[cursor..position]).count() as u64;
                let start =
                    memrchr(b'\n', &data[cursor..position]).map_or(cursor, |n| cursor + n + 1);
                let end = memchr(b'\n', &data[position..]).map_or(data.len(), |n| position + n);
                let text = strip_cr(&data[start..end], end < data.len());
                if position + needle_len <= start + text.len()
                    && !self.matched(
                        path,
                        *line,
                        offset + start as u64,
                        position - start..position - start + needle_len,
                        text,
                        count,
                    )
                {
                    return false;
                }
                *line += 1;
                cursor = (end + 1).min(data.len());
            }
            *line += memchr_iter(b'\n', &data[cursor..]).count() as u64;
        } else {
            let mut start = 0;
            while start < data.len() {
                if self.stopped() {
                    return false;
                }
                let end = memchr(b'\n', &data[start..]).map_or(data.len(), |n| start + n);
                let text = strip_cr(&data[start..end], end < data.len());
                if let Some(range) = self.matcher.as_ref().unwrap().find(text)
                    && !self.matched(path, *line, offset + start as u64, range, text, count)
                {
                    return false;
                }
                *line += 1;
                start = end + 1;
            }
        }
        true
    }
}

fn strip_cr(text: &[u8], has_newline: bool) -> &[u8] {
    if has_newline {
        text.strip_suffix(b"\r").unwrap_or(text)
    } else {
        text
    }
}
