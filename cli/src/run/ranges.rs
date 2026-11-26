//! \[DESIGN\] A file to import split for `--jobs` (RULES.md D1148): into
//! contiguous ranges of bytes, about equal, each beginning just after a
//! line break, the first at the file's first record, so each job imports
//! a run of whole records of its own, and the jobs' records together are
//! the file's, each once. A header, if the file has one, is kept apart,
//! for each job to read first.
//!
//! Splitting reads only the header and, from each place a range is to
//! begin, up to the next line break: the jobs begin at once, and the file
//! is read in their ranges side by side. Of CSV, a range that would begin
//! within a quoted field, after a line break the field holds, is found
//! before any row is imported, by the quotes of each range, counted side
//! by side: a range begins within a quoted field just when the quotes
//! before it are odd in number. That import fails, as a record split
//! between two jobs cannot be read whole by either: one job reads the
//! file whole. The lines and records before a range are counted only
//! when a record of a job is told, so it is told by its place in the
//! file, not in its range.

use std::io::{self, Read, Seek, SeekFrom};

/// [RFC 4180 §2 rule 1] What ends a line.
const LF: u8 = b'\n';
/// [RFC 4180 §2 rules 5-7] What encloses a field.
const QUOTE: u8 = b'"';
/// \[DESIGN\] The bytes read at a time: a few pages, so the read costs
/// little beside the bytes looked at.
const CHUNK: usize = 64 * 1024;

/// A range of a file one job imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// Where it begins, in bytes from the file's start.
    pub start: u64,
    /// Where it ends, the byte after its last.
    pub end: u64,
}

/// What comes before a range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Before {
    /// How many lines, a header's among them.
    pub lines: u64,
    /// How many records of data, a header not counted.
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

/// What a read of the file from its start has seen.
#[derive(Default)]
struct Scan {
    /// Whether a field may be enclosed in quotes, so they are counted.
    quotes: bool,
    /// Whether it is within a quoted field.
    quoted: bool,
    lines: u64,
    /// The line breaks outside quoted fields: each ends a record.
    records: u64,
}

impl Scan {
    /// Reads `bytes` through the first line break outside a quoted field
    /// if `first`: where it stopped, or none if it read them all.
    fn read(&mut self, bytes: &[u8], first: bool) -> Option<usize> {
        for (n, &b) in bytes.iter().enumerate() {
            if self.quotes && b == QUOTE {
                self.quoted = !self.quoted;
            }
            if b != LF {
                continue;
            }
            self.lines = self.lines.saturating_add(1);
            self.records = self.records.saturating_add(u64::from(!self.quoted));
            if first && !self.quoted {
                return Some(n.saturating_add(1));
            }
        }
        None
    }
}

/// The header of `input`, read from its start, quotes counted if
/// `quoted`: its bytes, its line break too, and its lines.
fn header<R: Read>(input: &mut R, quoted: bool) -> io::Result<(Vec<u8>, u64)> {
    let (mut buffer, mut scan, mut header) = (vec![0; CHUNK], Scan { quotes: quoted, ..Scan::default() }, Vec::new());
    loop {
        let n = input.read(&mut buffer)?;
        let bytes = buffer.get(..n).unwrap_or_default();
        let found = scan.read(bytes, true);
        header.extend_from_slice(bytes.get(..found.unwrap_or(n)).unwrap_or_default());
        if n == 0 || found.is_some() {
            return Ok((header, scan.lines));
        }
    }
}

/// Where the first line to begin at or after `from` of `input` begins:
/// just after the first line break there; none if there is none.
fn line_start<R: Read + Seek>(input: &mut R, from: u64) -> io::Result<Option<u64>> {
    input.seek(SeekFrom::Start(from))?;
    let (mut buffer, mut at) = (vec![0; CHUNK], from);
    loop {
        let n = input.read(&mut buffer)?;
        let bytes = buffer.get(..n).unwrap_or_default();
        if n == 0 {
            return Ok(None);
        }
        if let Some(p) = bytes.iter().position(|&b| b == LF) {
            let p = u64::try_from(p).unwrap_or(u64::MAX);
            return Ok(Some(at.saturating_add(p).saturating_add(1)));
        }
        at = at.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
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
/// records each once. Whether a range begins within a quoted field is
/// not looked at here (`straddles`).
///
/// # Errors
///
/// The file cannot be read.
pub fn split<R: Read + Seek>(input: &mut R, len: u64, jobs: usize, written: Written) -> Result<Split, String> {
    let unread = |e: io::Error| format!("cannot read: {e}");
    let (header, header_lines) = if written.header { header(input, written.quoted).map_err(unread)? } else { (Vec::new(), 0) };
    let first = u64::try_from(header.len()).unwrap_or(u64::MAX);
    let mut starts = vec![first];
    for k in 1..jobs {
        let last = starts.last().copied().unwrap_or(first);
        match line_start(input, target(first, len, (k, jobs)).max(last)).map_err(unread)? {
            Some(start) if start < len => starts.push(start),
            Some(_) | None => break,
        }
    }
    let mut ranges: Vec<Range> = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n.saturating_add(1)).copied().unwrap_or(len);
        if end > start {
            ranges.push(Range { start, end });
        }
    }
    Ok(Split { header, header_lines, ranges })
}

/// How many quotes `range` of `input` holds.
///
/// # Errors
///
/// The file cannot be read.
pub fn quotes<R: Read + Seek>(input: &mut R, range: Range) -> io::Result<u64> {
    input.seek(SeekFrom::Start(range.start))?;
    let mut left = input.take(range.end.saturating_sub(range.start));
    let (mut buffer, mut count) = (vec![0; CHUNK], 0_u64);
    loop {
        let n = left.read(&mut buffer)?;
        if n == 0 {
            return Ok(count);
        }
        let bytes = buffer.get(..n).unwrap_or_default();
        // Each byte adds one or none: the count stays below the bytes read.
        count = bytes.iter().fold(count, |sum, &b| sum.wrapping_add(u64::from(b == QUOTE)));
    }
}

/// [RFC 4180 §2 rules 5-7] Whether a range after the first begins within
/// a quoted field, of the ranges' counts of `quotes`, in their order, the
/// header's quotes even: whether those before one are odd in number.
pub fn straddles(quotes: &[u64]) -> bool {
    let mut odd = false;
    quotes.split_last().is_some_and(|(_, before)| {
        before.iter().any(|&q| {
            odd ^= q % 2 == 1;
            odd
        })
    })
}

/// The lines and records before each of `starts`, in the file's order,
/// of `input` read from its start as `written` says.
///
/// # Errors
///
/// The file cannot be read.
pub fn before<R: Read>(input: &mut R, starts: &[u64], written: Written) -> io::Result<Vec<Before>> {
    let mut scan = Scan { quotes: written.quoted, ..Scan::default() };
    let (mut buffer, mut at, mut told) = (vec![0; CHUNK], 0_u64, Vec::with_capacity(starts.len()));
    for &start in starts {
        while at < start {
            let want = usize::try_from(start.saturating_sub(at)).unwrap_or(CHUNK).min(CHUNK);
            let n = input.read(buffer.get_mut(..want).unwrap_or_default())?;
            if n == 0 {
                break;
            }
            scan.read(buffer.get(..n).unwrap_or_default(), false);
            at = at.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        }
        told.push(Before { lines: scan.lines, records: scan.records.saturating_sub(u64::from(written.header)) });
    }
    Ok(told)
}

#[cfg(test)]
mod tests;
