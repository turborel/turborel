//! \[DESIGN\] A file to import split for `--jobs` (RULES.md D1148): into
//! contiguous ranges of bytes, about equal, each beginning just after a
//! line break, the first at the file's first record, so each job imports
//! a run of whole records of its own, and the jobs' records together are
//! the file's, each once. A header, if the file has one, is kept apart,
//! for each job to read first.
//!
//! The file is read once, front to back, to the last range's beginning:
//! it counts the lines and records before each range, so a job tells a
//! record by its place in the file, not in its range; and, of CSV, the
//! quotes, so a range that would begin within a quoted field, after a
//! line break the field holds, is found before any row is imported. That
//! import fails, as a record split between two jobs cannot be read whole
//! by either: one job reads the file whole.

use std::io::{self, Read};

/// [RFC 4180 §2 rule 1] What ends a line.
const LF: u8 = b'\n';
/// [RFC 4180 §2 rules 5-7] What encloses a field.
const QUOTE: u8 = b'"';
/// \[DESIGN\] The bytes read at a time: a few pages, so the read costs
/// little beside the bytes looked at.
const CHUNK: usize = 64 * 1024;

/// A range of a file one job imports, and what comes before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// Where it begins, in bytes from the file's start.
    pub start: u64,
    /// Where it ends, the byte after its last.
    pub end: u64,
    /// How many lines come before it, a header's among them.
    pub lines: u64,
    /// How many records of data come before it, a header not counted.
    pub records: u64,
}

/// A file split for its jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// The header's bytes, its line break too; none without one.
    pub header: Vec<u8>,
    /// How many lines the header has.
    pub header_lines: u64,
    /// The ranges, in the file's order, none empty.
    pub ranges: Vec<Range>,
}

/// How the file is written, as the split needs it.
#[derive(Debug, Clone, Copy)]
pub struct Written {
    /// Whether a field may be enclosed in quotes, and so hold a line
    /// break.
    pub quoted: bool,
    /// Whether the first record is a header.
    pub header: bool,
}

/// The message of a range that would begin within a quoted field.
pub const STRADDLES: &str = "a quoted field holds a line break where the file is split between jobs, so its record \
                             cannot be read whole by either: import it with --jobs 1";

/// Where a read of the file is, and what it has seen.
#[derive(Default)]
struct Scan {
    /// The bytes read.
    at: u64,
    /// Whether it is within a quoted field.
    quoted: bool,
    lines: u64,
    /// The line breaks outside quoted fields: each ends a record.
    records: u64,
}

impl Scan {
    /// Reads `bytes` up to and through the first line break at or after
    /// `from`, outside a quoted field if `outside`, quotes counted if
    /// `quoted`: where it ends, or none if there is none.
    fn through(&mut self, bytes: &[u8], from: u64, (quoted, outside): (bool, bool)) -> Option<usize> {
        for (n, &b) in bytes.iter().enumerate() {
            self.at = self.at.saturating_add(1);
            if quoted && b == QUOTE {
                self.quoted = !self.quoted;
            }
            if b != LF {
                continue;
            }
            self.lines = self.lines.saturating_add(1);
            self.records = self.records.saturating_add(u64::from(!self.quoted));
            if self.at > from && !(outside && self.quoted) {
                return Some(n.saturating_add(1));
            }
        }
        None
    }
}

/// The input read a chunk at a time, the bytes of the chunk not yet
/// scanned kept.
struct Chunks<'a, R: Read> {
    input: &'a mut R,
    buffer: Vec<u8>,
    held: std::ops::Range<usize>,
}

impl<R: Read> Chunks<'_, R> {
    /// The bytes not yet scanned, reading more if none are held; empty at
    /// the end.
    fn next(&mut self) -> io::Result<&[u8]> {
        if self.held.is_empty() {
            let n = self.input.read(&mut self.buffer)?;
            self.held = 0..n;
        }
        Ok(self.buffer.get(self.held.clone()).unwrap_or_default())
    }

    /// Takes `n` of the bytes held as scanned.
    fn scanned(&mut self, n: usize) {
        self.held.start = self.held.start.saturating_add(n).min(self.held.end);
    }
}

/// Reads `chunks` through the first line break at or after `from`, as
/// `Scan::through` does: whether there was one.
fn read_through<R: Read>(chunks: &mut Chunks<'_, R>, scan: &mut Scan, from: u64, how: (bool, bool)) -> io::Result<bool> {
    loop {
        let bytes = chunks.next()?;
        if bytes.is_empty() {
            return Ok(false);
        }
        let (len, found) = (bytes.len(), scan.through(bytes, from, how));
        chunks.scanned(found.unwrap_or(len));
        if found.is_some() {
            return Ok(true);
        }
    }
}

/// Where the `k`th of `jobs` ranges of the `len` bytes after `first` is
/// to begin, about.
fn target(first: u64, len: u64, (k, jobs): (usize, usize)) -> u64 {
    let span = u128::from(len.saturating_sub(first));
    let (k, jobs) = (u128::try_from(k).unwrap_or(0), u128::try_from(jobs).unwrap_or(1).max(1));
    let part = span.saturating_mul(k).checked_div(jobs).unwrap_or(0);
    first.saturating_add(u64::try_from(part).unwrap_or(u64::MAX))
}

/// `input`, a file of `len` bytes written as `written` says, split into
/// at most `jobs` ranges: none empty, the first beginning after the
/// header, each other just after a line break, together the file's
/// records each once.
///
/// # Errors
///
/// The file cannot be read; `STRADDLES` if a range would begin within a
/// quoted field.
pub fn split<R: Read>(input: &mut R, len: u64, jobs: usize, written: Written) -> Result<Split, String> {
    let unread = |e: io::Error| format!("cannot read: {e}");
    let mut chunks = Chunks { input, buffer: vec![0; CHUNK], held: 0..0 };
    let mut scan = Scan::default();
    let mut header = Vec::new();
    if written.header {
        loop {
            let bytes = chunks.next().map_err(unread)?;
            if bytes.is_empty() {
                break;
            }
            let found = scan.through(bytes, 0, (written.quoted, true));
            let taken = found.unwrap_or(bytes.len());
            header.extend_from_slice(bytes.get(..taken).unwrap_or_default());
            chunks.scanned(taken);
            if found.is_some() {
                break;
            }
        }
    }
    let header_lines = scan.lines;
    let first = Range { start: scan.at, end: len, lines: scan.lines, records: 0 };
    let mut starts = vec![first];
    for k in 1..jobs {
        let from = target(first.start, len, (k, jobs)).max(scan.at);
        if !read_through(&mut chunks, &mut scan, from, (written.quoted, false)).map_err(unread)? {
            break;
        }
        if scan.quoted {
            return Err(STRADDLES.to_owned());
        }
        let records = scan.records.saturating_sub(u64::from(written.header));
        starts.push(Range { start: scan.at, end: len, lines: scan.lines, records });
    }
    let mut ranges: Vec<Range> = Vec::with_capacity(starts.len());
    for (n, range) in starts.iter().enumerate() {
        let end = starts.get(n.saturating_add(1)).map_or(len, |next| next.start);
        if end > range.start {
            ranges.push(Range { end, ..*range });
        }
    }
    Ok(Split { header, header_lines, ranges })
}

#[cfg(test)]
mod tests;
