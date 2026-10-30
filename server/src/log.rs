//! \[DESIGN\] The server's diagnostics sink (RULES.md D40, D42): each event
//! a line of JSON, stamped with the time it was taken, in UTC to the
//! millisecond (RFC 3339), with its level and its kind, then its fields
//! (`log/json.rs`): a program routes and filters the lines, a person reads
//! them. They go to standard error, or are appended to a file.
//!
//! ```text
//! {"time":"2026-10-27T20:15:56.701Z","level":"warning","event":"slow_statement","user":"ANN","millis":2400,"statement":"SELECT ..."}
//! ```
//!
//! The database never waits on its diagnostics: an event is made a line
//! where it is told and queued, and a thread of its own,
//! `turborel-server-log` (`runtime.rs`), writes the lines queued, flushing
//! once the queue is empty. A slow disk or a blocked pipe so holds up no
//! statement: past `QUEUE_LINES` waiting, a line is dropped and counted,
//! and the count told, a line of its own, as the writer next writes. A
//! line that cannot be written is lost.
//!
//! A file is opened again when the operator asks (`Reopener`), as a log
//! rotated asks: moved away, then the server told; the lines after go to a
//! new file at the same path.

mod json;

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::SystemTime;

pub use json::line;
use turborel::diagnostics::{Diagnostics, Event};
use turborel::files::Noted;

/// \[DESIGN\] The lines waiting for the writer, at most: past them, a line
/// is dropped rather than its event's teller made to wait.
const QUEUE_LINES: usize = 1024;

/// What the writer is given.
enum Message {
    /// A line to write.
    Line(String),
    /// The file its lines are counted writes of (RULES.md D56).
    Noting(Noted),
    /// The file to be opened again, before the lines after.
    Reopen,
    /// The lines are done with: written, it ends.
    Stop,
}

/// What the sink, its reopeners and its writer share.
#[derive(Default)]
struct Shared {
    /// Lines dropped since the writer last told of them.
    dropped: AtomicU64,
    /// Whether the file is to be opened again, asked while the queue was
    /// full.
    reopen: AtomicBool,
}

/// \[DESIGN\] A sink writing each event as a line of JSON.
pub struct Lines {
    queue: SyncSender<Message>,
    shared: Arc<Shared>,
    writer: Option<JoinHandle<()>>,
}

/// \[DESIGN\] What asks a sink to open its file again: any thread may.
#[derive(Clone)]
pub struct Reopener {
    queue: SyncSender<Message>,
    shared: Arc<Shared>,
}

impl Reopener {
    /// Asks the sink to open its file again, at the same path, before its
    /// next line: once a rotation has moved it away. Of a sink to standard
    /// error, nothing is done.
    pub fn reopen(&self) {
        // In turn, so the lines told before go to the file it had; a queue
        // full has a line coming, before which the flag is looked at.
        if let Err(TrySendError::Full(_)) = self.queue.try_send(Message::Reopen) {
            self.shared.reopen.store(true, Ordering::Release);
        }
    }
}

impl Lines {
    /// Lines written to `out`.
    ///
    /// # Errors
    ///
    /// The writer's thread cannot be made.
    pub fn new(out: Box<dyn Write + Send>) -> io::Result<Lines> {
        Lines::started(Target::Stream(out), QUEUE_LINES)
    }

    /// The lines, each counted a write of the file `noted` shows (RULES.md
    /// D56).
    #[must_use]
    pub fn noting(self, noted: Noted) -> Lines {
        // The queue is empty as the sink is made, so this waits for none.
        let _ = self.queue.send(Message::Noting(noted));
        self
    }

    /// Lines written to standard error.
    ///
    /// # Errors
    ///
    /// The writer's thread cannot be made.
    pub fn to_stderr() -> io::Result<Lines> {
        Lines::new(Box::new(io::stderr()))
    }

    /// Lines appended to the file at `path`, made if it is not there.
    ///
    /// # Errors
    ///
    /// The file cannot be opened for appending, or the writer's thread
    /// cannot be made.
    pub fn appending(path: &Path) -> io::Result<Lines> {
        Lines::started(Target::File(path.to_path_buf(), appended(path)?), QUEUE_LINES)
    }

    /// What asks it to open its file again.
    pub fn reopener(&self) -> Reopener {
        Reopener { queue: self.queue.clone(), shared: Arc::clone(&self.shared) }
    }

    /// Lines to `target`, at most `bound` waiting, its writer started.
    fn started(target: Target, bound: usize) -> io::Result<Lines> {
        let (queue, taken) = sync_channel(bound);
        let shared = Arc::new(Shared::default());
        let writer = Writer { target, noted: None, shared: Arc::clone(&shared) };
        let thread = crate::runtime::writer(move || writer.run(&taken))?;
        Ok(Lines { queue, shared, writer: Some(thread) })
    }
}

impl Diagnostics for Lines {
    fn event(&self, event: &Event) {
        match self.queue.try_send(Message::Line(line(SystemTime::now(), event))) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Drop for Lines {
    /// Writes the lines queued, then ends the writer.
    fn drop(&mut self) {
        let _ = self.queue.send(Message::Stop);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// The file at `path` opened for appending, made if it is not there.
fn appended(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    // \[DESIGN\] Made its user's alone, as the database's files are
    // (RULES.md D56).
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.create(true).append(true).open(path)
}

/// Where lines go.
enum Target {
    /// A stream: standard error, or a program's.
    Stream(Box<dyn Write + Send>),
    /// A file, and its path, to open it again by.
    File(PathBuf, File),
}

impl Target {
    fn out(&mut self) -> &mut dyn Write {
        match self {
            Target::Stream(out) => out,
            Target::File(_, file) => file,
        }
    }

    /// Its file opened again at its path; if it cannot be, the one it has
    /// kept, and why said in it.
    fn reopen(&mut self) {
        let Target::File(path, file) = self else { return };
        let _ = file.flush();
        match appended(path) {
            Ok(again) => *file = again,
            Err(e) => {
                let _ = writeln!(file, "{}", json::not_reopened(SystemTime::now(), &path.display().to_string(), &e.to_string()));
            }
        }
    }
}

/// The writer's own: where its lines go, and what it counts them to.
struct Writer {
    target: Target,
    noted: Option<Noted>,
    shared: Arc<Shared>,
}

impl Writer {
    /// Writes what `taken` gives until it is stopped, or every sender is
    /// gone; flushing once nothing more waits.
    fn run(mut self, taken: &Receiver<Message>) {
        while let Ok(first) = taken.recv() {
            let mut next = Some(first);
            while let Some(message) = next.take() {
                if !self.take(message) {
                    self.told_dropped();
                    let _ = self.target.out().flush();
                    return;
                }
                next = taken.try_recv().ok();
            }
            let _ = self.target.out().flush();
        }
    }

    /// Does what `message` asks: false once it is to stop.
    fn take(&mut self, message: Message) -> bool {
        if self.shared.reopen.swap(false, Ordering::AcqRel) {
            self.target.reopen();
        }
        match message {
            Message::Line(text) => {
                self.write(&text);
                self.told_dropped();
            }
            Message::Noting(noted) => self.noted = Some(noted),
            Message::Reopen => self.target.reopen(),
            Message::Stop => return false,
        }
        true
    }

    /// Lines dropped since last told, told.
    fn told_dropped(&mut self) {
        let dropped = self.shared.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            self.write(&json::dropped(SystemTime::now(), dropped));
        }
    }

    fn write(&mut self, text: &str) {
        let _ = writeln!(self.target.out(), "{text}");
        if let Some(noted) = &self.noted {
            noted.wrote();
        }
    }
}

#[cfg(test)]
mod tests;
