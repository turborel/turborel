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
use std::sync::{Condvar, Mutex, PoisonError};
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

/// Whether the file's ranges may be read as they are, told once their
/// quotes are counted, beside the jobs (`counted`): or why not.
#[derive(Default)]
struct Verdict {
    told: Mutex<Option<Result<(), String>>>,
    ready: Condvar,
}

impl Verdict {
    /// Tells `verdict` to every job waiting for it.
    fn tell(&self, verdict: Result<(), String>) {
        *self.told.lock().unwrap_or_else(PoisonError::into_inner) = Some(verdict);
        self.ready.notify_all();
    }

    /// The verdict, once told.
    fn wait(&self) -> Result<(), String> {
        let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(verdict) = told.as_ref() {
                return verdict.clone();
            }
            told = self.ready.wait(told).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A source a job reads, which gives no record once another job failed,
/// or Ctrl-C was pressed; nor, until the verdict on the ranges is told,
/// the record whose reading would commit its first batch (`gate`).
struct Stoppable<'a, R: Source> {
    source: R,
    stop: &'a AtomicBool,
    /// What hears Ctrl-C, and how many times it was pressed before.
    pressed: Option<(&'a Interrupts, u64)>,
    /// The verdict, and how many records may be read before it is told.
    gate: Option<(&'a Verdict, u64)>,
    /// How many records it gave.
    read: u64,
    /// Whether it gave none for another's failure.
    stopped: bool,
}

/// A source's failure of `message`.
fn failed(message: &str) -> SourceError {
    SourceError { position: Position::default(), message: message.to_owned() }
}

impl<R: Source> Stoppable<'_, R> {
    /// Fails if Ctrl-C was pressed, or another job failed; waits for the
    /// verdict before a record its gate holds, failing as it says.
    fn go_on(&mut self) -> Result<(), SourceError> {
        if self.pressed.is_some_and(|(interrupts, before)| interrupts.presses() != before) {
            return Err(failed(CANCELLED));
        }
        if self.stop.load(Ordering::Relaxed) {
            self.stopped = true;
            return Err(failed(STOPPED));
        }
        if self.gate.is_some_and(|(_, at)| self.read >= at) {
            self.passed()?;
        }
        Ok(())
    }

    /// Waits for the verdict, failing as it says; then holds no record.
    fn passed(&mut self) -> Result<(), SourceError> {
        if let Some((verdict, _)) = self.gate.take() {
            verdict.wait().map_err(|why| failed(&why))?;
        }
        Ok(())
    }

    /// `read`, a record given or the end: the end only once the verdict
    /// is told, as an import's last batch commits at the end.
    fn given<T>(&mut self, read: Result<Option<T>, SourceError>) -> Result<Option<T>, SourceError> {
        match read? {
            Some(record) => {
                self.read = self.read.saturating_add(1);
                Ok(Some(record))
            }
            None => self.passed().map(|()| None),
        }
    }
}

impl<R: Source> Source for Stoppable<'_, R> {
    fn headings(&mut self) -> Result<Option<Vec<Heading>>, SourceError> {
        self.source.headings()
    }

    fn record(&mut self) -> Result<Option<Record>, SourceError> {
        self.go_on()?;
        let read = self.source.record();
        self.given(read)
    }

    fn splits(&self) -> bool {
        self.source.splits()
    }

    fn record_into(&mut self, cells: &mut Cells) -> Result<Option<Position>, SourceError> {
        self.go_on()?;
        let read = self.source.record_into(cells);
        self.given(read)
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
    /// The verdict on the ranges, if their quotes are counted.
    verdict: Option<&'a Verdict>,
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
            // [DESIGN] RULES.md D1148: a batch commits as its last record
            // is read, so the record before which nothing has committed is
            // held until the verdict; of a batch of all, the end.
            let at = shared.into.batch.checked_sub(1).unwrap_or(u64::MAX);
            let gate = shared.verdict.map(|verdict| (verdict, at));
            let mut source = Stoppable { source, stop: shared.stop, pressed: shared.pressed, gate, read: 0, stopped: false };
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
        if let Some(verdict) = shared.verdict {
            let (path, ranges) = (shared.path, &shared.split.ranges);
            let counting = std::thread::Builder::new().name(format!("{THREAD}-quotes")).spawn_scoped(scope, move || {
                verdict.tell(counted(path, ranges));
            });
            if counting.is_err() {
                verdict.tell(Err("[DESIGN] the quotes of the file's ranges could not be counted".to_owned()));
            }
        }
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

/// Whether the `ranges` of the file at `path` may be imported as they
/// are: none after the first beginning within a quoted field.
///
/// # Errors
///
/// `STRADDLES` if one does; the file cannot be read.
fn counted(path: &Path, ranges: &[Range]) -> Result<(), String> {
    if ranges::straddles(&quotes_of(path, ranges)?) { Err(ranges::STRADDLES.to_owned()) } else { Ok(()) }
}

/// The import of `done`, refused for `why` as its ranges cannot be read
/// as they are: a job begun within a quoted field read what is no record,
/// so the jobs' failures are not told, only their rows committed and
/// rolled back.
fn refused(done: &[Done], why: &str) -> ImportError {
    let (committed, rolled_back) = done.iter().fold((0_u64, 0_u64), |(c, r), d| match &d.result {
        Ok(imported) => (c.saturating_add(imported.rows), r),
        Err(e) => (c.saturating_add(e.committed), r.saturating_add(e.rolled_back)),
    });
    ImportError { position: None, cause: Cause::Input(format!("[DESIGN] {why}")), committed, rolled_back }
}

/// The ranges of the file at `path` for `jobs` jobs, read as `dialect`
/// says. Whether one begins within a quoted field is found beside the
/// jobs (`counted`).
///
/// # Errors
///
/// The file is standard input, or not a regular file, or cannot be read.
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
    ranges::split(&mut { file }, metadata.len(), jobs, written(dialect))
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
        let counting = Verdict::default();
        let verdict = (dialect.is_quoted() && split.ranges.len() > 1).then_some(&counting);
        let shared = Shared { path, split: &split, dialect, into, stop: &stop, pressed, verdict };
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
        if let Some(Err(why)) = verdict.map(Verdict::wait) {
            return self.imported(&shown, Err(refused(&done, &why)));
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
        let pressed = Some((&interrupts, interrupts.presses()));
        let mut job = Stoppable { source, stop: &stop, pressed, gate: None, read: 0, stopped: false };
        assert!(job.record().unwrap().is_some(), "a press before it began");
        interrupts.press();
        assert_eq!(job.record().unwrap_err().message, CANCELLED);
        assert!(!job.stopped, "cancelled of itself");
        job.pressed = None;
        stop.store(true, Ordering::Relaxed);
        assert_eq!(job.record().unwrap_err().message, STOPPED);
        assert!(job.stopped);
    }

    /// Verifies: [DESIGN] RULES.md D1148: a job gives the records before
    /// its gate at once, and the one at it, or the end, only once the
    /// verdict on the ranges is told: failing as it says, else reading on.
    #[test]
    fn a_job_waits_for_the_verdict_before_a_commit() {
        let stop = AtomicBool::new(false);
        let job = |verdict: &'static Verdict, at: u64| {
            let source = Delimited::new(Cursor::new(b"1\n2\n3\n".to_vec()), Dialect::csv());
            Stoppable { source, stop: &stop, pressed: None, gate: Some((verdict, at)), read: 0, stopped: false }
        };
        let refused: &'static Verdict = Box::leak(Box::default());
        refused.tell(Err(ranges::STRADDLES.to_owned()));
        let mut held = job(refused, 2);
        assert!(held.record().unwrap().is_some() && held.record().unwrap().is_some(), "before its gate");
        assert_eq!(held.record().unwrap_err().message, ranges::STRADDLES);
        let mut ended = job(refused, u64::MAX);
        for _ in 0_u32..3 {
            assert!(ended.record().unwrap().is_some());
        }
        assert_eq!(ended.record().unwrap_err().message, ranges::STRADDLES, "the end waits too");
        let passed: &'static Verdict = Box::leak(Box::default());
        let mut reads = job(passed, 0);
        std::thread::scope(|scope| {
            scope.spawn(|| passed.tell(Ok(())));
            assert!(reads.record().unwrap().is_some(), "once told");
        });
        assert!(reads.gate.is_none() && reads.record().unwrap().is_some());
    }
}
