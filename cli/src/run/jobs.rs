//! \[DESIGN\] `--jobs N` (RULES.md D1148): one file imported over N
//! connections at once, each its own import of a contiguous range of the
//! file (`ranges.rs`), on a thread of its own, so the server loads the
//! ranges side by side. Each job reads the file's header first, if it has
//! one, then its range; each record it tells is told by its place in the
//! file. The rows said are the jobs' together. A job that fails stops the
//! others at their next record, each committing nothing more; the import
//! is told as the first of them in the file's order that failed of
//! itself, with the rows every job committed and rolled back.
//!
//! The jobs connect to the server the command line names, each proved as
//! the first connection was. Standard input cannot be read in ranges, so
//! a file that is not a regular one is refused.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use turborel_client::agent::Connected;
use turborel_client::import::{
    self, Cause, Cells, Delimited, Dialect, Heading, Import, ImportError, Imported, Position, Record, Rejected, Source, SourceError,
};

use super::ranges::{self, Range, Split, Written};
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

/// A source a job reads, which gives no record once another job failed.
struct Stoppable<'a, R: Source> {
    source: R,
    stop: &'a AtomicBool,
    /// Whether it gave none for that.
    stopped: bool,
}

impl<R: Source> Stoppable<'_, R> {
    /// Fails if another job failed.
    fn go_on(&mut self) -> Result<(), SourceError> {
        if self.stop.load(Ordering::Relaxed) {
            self.stopped = true;
            return Err(SourceError { position: Position::default(), message: STOPPED.to_owned() });
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
}

/// `position`, of a job's records, as of the file's: after the records
/// and lines before `range`, its header's lines, `header_lines`, read
/// again by the job, not counted twice.
fn placed(position: Position, range: &Range, header_lines: u64) -> Position {
    let line = position.line.map(|l| l.saturating_sub(header_lines).saturating_add(range.lines));
    Position { record: position.record.saturating_add(range.records), line }
}

/// An import's failure of `msg`, before any row.
fn unbegun(msg: String) -> ImportError {
    ImportError { position: None, cause: Cause::Input(msg), committed: 0, rolled_back: 0 }
}

/// Imports `job`'s range: what it did, and its connection back.
fn run(mut job: Job, shared: &Shared<'_>) -> (Done, Connected) {
    let unread = |e: std::io::Error| unbegun(format!("cannot read: {e}"));
    let opened = File::open(shared.path).and_then(|mut f| f.seek(SeekFrom::Start(job.range.start)).map(|_| f)).map_err(unread);
    let (result, rejected, stopped) = match opened {
        Ok(file) => {
            let range = file.take(job.range.end.saturating_sub(job.range.start));
            let input = BufReader::with_capacity(READ_BYTES, Cursor::new(shared.split.header.clone()).chain(range));
            let mut source = Stoppable { source: Delimited::new(input, shared.dialect.clone()), stop: shared.stop, stopped: false };
            let mut rejected = Vec::new();
            let result = import::import(&mut job.connection, &mut source, shared.into, &mut |row| rejected.push(row));
            (result, rejected, source.stopped)
        }
        Err(e) => (Err(e), Vec::new(), false),
    };
    if result.is_err() && !stopped {
        shared.stop.store(true, Ordering::Relaxed);
    }
    let lines = shared.split.header_lines;
    let rejected = rejected.into_iter().map(|r| Rejected { position: placed(r.position, &job.range, lines), ..r }).collect();
    let result = result.map_err(|e| ImportError { position: e.position.map(|p| placed(p, &job.range, lines)), ..e });
    (Done { result, rejected, stopped }, job.connection)
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
    let written = Written { quoted: dialect.is_quoted(), header: dialect.has_header() };
    ranges::split(&mut BufReader::with_capacity(READ_BYTES, file), metadata.len(), jobs, written)
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
        for range in &split.ranges {
            match self.connection.spare() {
                Ok(connection) => connected.push(Job { range: *range, connection }),
                Err(failure) => {
                    let begun = ImportError { position: None, cause: Cause::Sql(Box::new(failure)), committed: 0, rolled_back: 0 };
                    return self.imported(&shown, Err(begun));
                }
            }
        }
        let stop = AtomicBool::new(false);
        let shared = Shared { path, split: &split, dialect, into, stop: &stop };
        let (done, connections): (Vec<Done>, Vec<Option<Connected>>) = run_all(connected, &shared).into_iter().unzip();
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
        let range = Range { start: 40, end: 80, lines: 6, records: 4 };
        assert_eq!(placed(Position { record: 2, line: Some(3) }, &range, 1), Position { record: 6, line: Some(8) });
        assert_eq!(placed(Position { record: 1, line: None }, &range, 0), Position { record: 5, line: None });
    }
}
