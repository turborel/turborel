//! \[DESIGN\] `--jobs N` (RULES.md D1148): one file imported over N
//! connections at once, each its own import of a contiguous range of the
//! file (`ranges.rs`), on a thread of its own, so the server loads the
//! ranges side by side. Each job reads the file's header first, if it has
//! one, then its range; each record it tells is told by its place in the
//! file. The rows said are the jobs' together. A job that fails stops the
//! others at their next record, each committing nothing more; the import
//! is told as the first of them in the file's order that failed of
//! itself, with the rows every job committed and rolled back. Ctrl-C
//! cancels each job's statement, and stops each at its next record: the
//! import is told once, as cancelled.
//!
//! The jobs connect to the server the command line names, each proved as
//! the first connection was, a password typed for it not asked for again.
//! Standard input cannot be read in ranges, so a file that is not a
//! regular one is refused.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use turborel_client::agent::Connected;
use turborel_client::import::{
    self, Cause, Cells, Delimited, Dialect, Heading, Import, ImportError, Imported, Position, Record, Rejected, Source, SourceError,
};

use crate::interrupt::Interrupts;

use super::ranges::{self, Before, Range, Split, Written};
use super::{Next, Shell, Statements};

/// What standard input is named as a file.
const STDIN: &str = "/dev/stdin";
/// What each job's thread is named, its number after.
const THREAD: &str = "turborel-import";
/// \[DESIGN\] The bytes each job reads from its range at a time: large
/// enough that a read's cost is shared by many records.
const READ_BYTES: usize = 1024 * 1024;
/// Why a job stopped that did not fail of itself.
const STOPPED: &str = "[DESIGN] stopped, as another job of the import failed";
/// Why a job stopped at Ctrl-C.
const CANCELLED: &str = "[DESIGN] cancelled, as Ctrl-C was pressed";

/// A source a job reads, which gives no record once another job failed,
/// or Ctrl-C was pressed.
struct Stoppable<'a, R: Source> {
    source: R,
    stop: &'a AtomicBool,
    /// What hears Ctrl-C, and how many times it was pressed before.
    pressed: Option<(&'a Interrupts, u64)>,
    /// Whether it gave none for another's failure.
    stopped: bool,
}

impl<R: Source> Stoppable<'_, R> {
    /// Fails if Ctrl-C was pressed, or another job failed.
    fn go_on(&mut self) -> Result<(), SourceError> {
        let fail = |message: &str| Err(SourceError { position: Position::default(), message: message.to_owned() });
        if self.pressed.is_some_and(|(interrupts, before)| interrupts.presses() != before) {
            return fail(CANCELLED);
        }
        if self.stop.load(Ordering::Relaxed) {
            self.stopped = true;
            return fail(STOPPED);
        }
        Ok(())
    }
}

impl<R: Source> Source for Stoppable<'_, R> {
    fn headings(&mut self) -> Result<Option<Vec<Heading>>, SourceError> {
        self.source.headings()
    }

    fn record(&mut self) -> Result<Option<Record>, SourceError> {
        self.go_on()?;
        self.source.record()
    }

    fn splits(&self) -> bool {
        self.source.splits()
    }

    fn record_into(&mut self, cells: &mut Cells) -> Result<Option<Position>, SourceError> {
        self.go_on()?;
        self.source.record_into(cells)
    }
}

/// A job: its range, and the connection it imports over.
struct Job {
    range: Range,
    connection: Connected,
}

/// What a job did: what it imported, or why it stopped; the rows it
/// skipped, placed in the file; and whether it stopped for another's
/// failure.
struct Done {
    result: Result<Imported, ImportError>,
    rejected: Vec<Rejected>,
    stopped: bool,
}

/// What a job is given besides its own: the file, its split, how it is
/// written and where its rows go, and what stops it.
struct Shared<'a> {
    path: &'a Path,
    split: &'a Split,
    dialect: &'a Dialect,
    into: &'a Import,
    stop: &'a AtomicBool,
    pressed: Option<(&'a Interrupts, u64)>,
}

/// `position`, of a job's records, as of the file's: after the records
/// and lines `before` its range, its header's lines, `header_lines`, read
/// again by the job, not counted twice.
fn placed(position: Position, before: Before, header_lines: u64) -> Position {
    let line = position.line.map(|l| l.saturating_sub(header_lines).saturating_add(before.lines));
    Position { record: position.record.saturating_add(before.records), line }
}

/// An import's failure of `msg`, before any row.
fn unbegun(msg: String) -> ImportError {
    ImportError { position: None, cause: Cause::Input(msg), committed: 0, rolled_back: 0 }
}

/// Imports `job`'s range: what it did, its records told by their place in
/// the range, and its connection back.
fn run(mut job: Job, shared: &Shared<'_>) -> (Done, Connected) {
    let unread = |e: std::io::Error| unbegun(format!("cannot read: {e}"));
    let opened = File::open(shared.path).and_then(|mut f| f.seek(SeekFrom::Start(job.range.start)).map(|_| f)).map_err(unread);
    let (result, rejected, stopped) = match opened {
        Ok(file) => {
            let range = file.take(job.range.end.saturating_sub(job.range.start));
            let input = BufReader::with_capacity(READ_BYTES, Cursor::new(shared.split.header.clone()).chain(range));
            let source = Delimited::new(input, shared.dialect.clone());
            let mut source = Stoppable { source, stop: shared.stop, pressed: shared.pressed, stopped: false };
            let mut rejected = Vec::new();
            let result = import::import(&mut job.connection, &mut source, shared.into, &mut |row| rejected.push(row));
            (result, rejected, source.stopped)
        }
        Err(e) => (Err(e), Vec::new(), false),
    };
    if result.is_err() && !stopped {
        shared.stop.store(true, Ordering::Relaxed);
    }
    (Done { result, rejected, stopped }, job.connection)
}

/// The records `done` tells, each job's of its range, told by their place
/// in the file at `path`, read as `written` says, its ranges those of
/// `split`. The lines and records before a range are counted only if a
/// job after the first tells one; if the file cannot be read for them,
/// each is told by its place in its range.
fn place(done: &mut [Done], path: &Path, split: &Split, written: Written) {
    let tells = |d: &Done| !d.rejected.is_empty() || d.result.as_ref().is_err_and(|e| e.position.is_some());
    let Some(last) = done.iter().rposition(tells).filter(|&last| last > 0) else { return };
    let starts: Vec<u64> = split.ranges.iter().take(last.saturating_add(1)).map(|r| r.start).collect();
    let counted = File::open(path).and_then(|file| ranges::before(&mut BufReader::with_capacity(READ_BYTES, file), &starts, written));
    let (Ok(befores), lines) = (counted, split.header_lines) else { return };
    for (d, before) in done.iter_mut().zip(befores) {
        for r in &mut d.rejected {
            r.position = placed(r.position, before, lines);
        }
        if let Err(e) = &mut d.result {
            e.position = e.position.map(|p| placed(p, before, lines));
        }
    }
}

/// Runs `jobs`, each on a thread of its own: what each did, in their
/// order, and their connections.
fn run_all(jobs: Vec<Job>, shared: &Shared<'_>) -> Vec<(Done, Option<Connected>)> {
    std::thread::scope(|scope| {
        let started: Vec<_> = jobs
            .into_iter()
            .enumerate()
            .map(|(n, job)| std::thread::Builder::new().name(format!("{THREAD}-{n}")).spawn_scoped(scope, move || run(job, shared)))
            .collect();
        started
            .into_iter()
            .map(|handle| {
                if let Ok(Ok((done, connection))) = handle.map(std::thread::ScopedJoinHandle::join) {
                    return (done, Some(connection));
                }
                // A thread that could not start, or panicked, fails the
                // import, and stops the other jobs.
                shared.stop.store(true, Ordering::Relaxed);
                let failed = unbegun("[DESIGN] a job of the import could not run".to_owned());
                (Done { result: Err(failed), rejected: Vec::new(), stopped: false }, None)
            })
            .collect()
    })
}

/// What the jobs `done` did together: the rows they imported and skipped,
/// or the first failure of itself, in the file's order, with every job's
/// rows committed and rolled back.
fn together(done: &[Done]) -> Result<Imported, ImportError> {
    let mut sum = Imported::default();
    let mut rolled_back = 0_u64;
    for d in done {
        match &d.result {
            Ok(imported) => {
                sum.rows = sum.rows.saturating_add(imported.rows);
                sum.rejected = sum.rejected.saturating_add(imported.rejected);
            }
            Err(e) => {
                sum.rows = sum.rows.saturating_add(e.committed);
                rolled_back = rolled_back.saturating_add(e.rolled_back);
            }
        }
    }
    let first = done.iter().filter(|d| !d.stopped).find_map(|d| d.result.as_ref().err());
    match first {
        Some(e) => Err(ImportError { committed: sum.rows, rolled_back, ..e.clone() }),
        None => Ok(sum),
    }
}

/// How many quotes each of `ranges` of the file at `path` holds, each
/// counted on a thread of its own, side by side.
///
/// # Errors
///
/// The file cannot be read.
fn quotes_of(path: &Path, ranges: &[Range]) -> Result<Vec<u64>, String> {
    let count = |range: Range| File::open(path).and_then(|mut file| ranges::quotes(&mut file, range));
    std::thread::scope(|scope| {
        let started: Vec<_> = ranges
            .iter()
            .enumerate()
            .map(|(n, &range)| std::thread::Builder::new().name(format!("{THREAD}-{n}")).spawn_scoped(scope, move || count(range)))
            .collect();
        started
            .into_iter()
            .map(|handle| match handle.map(std::thread::ScopedJoinHandle::join) {
                Ok(Ok(counted)) => counted.map_err(|e| format!("cannot read: {e}")),
                Ok(Err(_)) | Err(_) => Err("[DESIGN] a job of the import could not run".to_owned()),
            })
            .collect()
    })
}

/// The ranges of the file at `path` for `jobs` jobs, read as `dialect`
/// says.
///
/// # Errors
///
/// The file is standard input, or not a regular file, or cannot be read;
/// a range would begin within a quoted field.
fn split_file(path: &Path, dialect: &Dialect, jobs: usize) -> Result<Split, String> {
    let unranged = || {
        "--jobs above 1 reads a file in ranges, and standard input, or a file that is not a regular one, cannot be: \
         import it with --jobs 1"
            .to_owned()
    };
    if path == Path::new(STDIN) {
        return Err(unranged());
    }
    let file = File::open(path).map_err(|e| format!("cannot read: {e}"))?;
    let metadata = file.metadata().map_err(|e| format!("cannot read: {e}"))?;
    if !metadata.is_file() {
        return Err(unranged());
    }
    let split = ranges::split(&mut { file }, metadata.len(), jobs, written(dialect))?;
    if dialect.is_quoted() && split.ranges.len() > 1 && ranges::straddles(&quotes_of(path, &split.ranges)?) {
        return Err(ranges::STRADDLES.to_owned());
    }
    Ok(split)
}

/// How a file written as `dialect` says is, as its split needs it.
fn written(dialect: &Dialect) -> Written {
    Written { quoted: dialect.is_quoted(), header: dialect.has_header() }
}

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// Imports the rows of the file at `path` into the table `into`
    /// names, read as `dialect` says, over `jobs` connections at once,
    /// each of its own range of the file: what was imported said, or the
    /// failure told.
    pub(super) fn import_jobs(&mut self, path: &Path, into: &Import, dialect: &Dialect, jobs: usize) -> Next {
        let shown = path.display().to_string();
        let split = match split_file(path, dialect, jobs) {
            Ok(split) => split,
            Err(msg) => return self.imported(&shown, Err(unbegun(format!("[DESIGN] {msg}")))),
        };
        let mut connected = Vec::with_capacity(split.ranges.len());
        let mut cancellers = Vec::with_capacity(split.ranges.len().saturating_add(1));
        cancellers.extend(self.connection.canceller());
        for range in &split.ranges {
            match self.connection.spare() {
                Ok(connection) => {
                    cancellers.extend(connection.canceller());
                    connected.push(Job { range: *range, connection });
                }
                Err(failure) => {
                    let begun = ImportError { position: None, cause: Cause::Sql(Box::new(failure)), committed: 0, rolled_back: 0 };
                    return self.imported(&shown, Err(begun));
                }
            }
        }
        let stop = AtomicBool::new(false);
        let pressed = self.interrupts.map(|interrupts| (interrupts, interrupts.presses()));
        if let Some(interrupts) = self.interrupts {
            interrupts.cancel_each(cancellers);
        }
        let shared = Shared { path, split: &split, dialect, into, stop: &stop, pressed };
        let (mut done, connections): (Vec<Done>, Vec<Option<Connected>>) = run_all(connected, &shared).into_iter().unzip();
        if let Some(interrupts) = self.interrupts {
            interrupts.cancel_by(self.connection.canceller());
        }
        place(&mut done, path, &split, written(dialect));
        for connection in connections.into_iter().flatten() {
            // The import is done; a session that does not end cleanly is
            // the server's to end.
            let _ = connection.terminate();
        }
        for row in done.iter().flat_map(|d| &d.rejected) {
            self.told(&format!("turborel: {shown}: {row}"));
        }
        self.imported(&shown, together(&done))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job's outcome: imported `rows`, or failed having committed them,
    /// stopped by another's failure if `stopped`.
    fn done(rows: u64, failed: Option<&str>, stopped: bool) -> Done {
        let result = match failed {
            None => Ok(Imported { rows, rejected: 1 }),
            Some(msg) => Err(ImportError { position: None, cause: Cause::Input(msg.to_owned()), committed: rows, rolled_back: 2 }),
        };
        Done { result, rejected: Vec::new(), stopped }
    }

    /// Verifies: [DESIGN] RULES.md D1148: the rows of an import's jobs are
    /// summed, those skipped too; a failure is the first, in the file's
    /// order, of a job that failed of itself, not one stopped for it, with
    /// every job's rows committed and rolled back.
    #[test]
    fn the_jobs_rows_are_summed() {
        assert_eq!(together(&[done(3, None, false), done(4, None, false)]), Ok(Imported { rows: 7, rejected: 2 }));
        let failed = together(&[done(3, Some(STOPPED), true), done(1, Some("bad"), false), done(5, None, false)]).unwrap_err();
        assert_eq!(failed.cause, Cause::Input("bad".to_owned()));
        assert_eq!((failed.committed, failed.rolled_back), (9, 4));
    }

    /// Verifies: [DESIGN] RULES.md D1148: a job's record is told by its
    /// place in the file: after the records and lines before its range,
    /// the header it reads again not counted twice.
    #[test]
    fn a_jobs_records_are_placed_in_the_file() {
        let before = Before { lines: 6, records: 4 };
        assert_eq!(placed(Position { record: 2, line: Some(3) }, before, 1), Position { record: 6, line: Some(8) });
        assert_eq!(placed(Position { record: 1, line: None }, before, 0), Position { record: 5, line: None });
    }

    /// Verifies: [DESIGN] RULES.md D46, D1148: a job reads no record once
    /// Ctrl-C was pressed after it began, failing of itself as cancelled;
    /// nor once another job failed, stopped for that.
    #[test]
    fn a_job_stops_at_ctrl_c_or_another_failing() {
        let (stop, interrupts) = (AtomicBool::new(false), Interrupts::default());
        interrupts.press();
        let source = Delimited::new(Cursor::new(b"1\n2\n3\n".to_vec()), Dialect::csv());
        let mut job = Stoppable { source, stop: &stop, pressed: Some((&interrupts, interrupts.presses())), stopped: false };
        assert!(job.record().unwrap().is_some(), "a press before it began");
        interrupts.press();
        assert_eq!(job.record().unwrap_err().message, CANCELLED);
        assert!(!job.stopped, "cancelled of itself");
        job.pressed = None;
        stop.store(true, Ordering::Relaxed);
        assert_eq!(job.record().unwrap_err().message, STOPPED);
        assert!(job.stopped);
    }
}
