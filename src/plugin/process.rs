//! \[DESIGN\] One plug-in process (RULES.md D170): started with
//! `std::process`, its standard input written by the caller, its standard
//! output and error read by threads of their own (`threads/pipes.rs`),
//! which hand on each frame, the memory it takes reserved first (D38),
//! and keep the last of what it wrote to its standard error.
//!
//! A process is stopped and waited for as it is dropped, so none outlives
//! its pool, whatever drops it.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::memory::{MemoryBudget, Reservation};
use crate::sync::lock_builtin;
use crate::threads::pipe_reader;

use super::errors::{ended, unavailable, violation};
use super::limits::Limits;
use super::wire::{HOST_CAPABILITIES, Hello, Message, Welcome, read_body, read_length, write_message};

/// The most bytes of a program's standard error kept: its last.
const TAIL_BYTES: usize = 4096;
/// The bytes the error pipe's reader reads at a time.
const ERROR_CHUNK: usize = 512;
/// The environment variables a program is given, of the database's: what
/// a program needs to find others and a temporary directory on the three
/// platforms, and nothing that may hold a secret.
const KEPT_ENVIRONMENT: [&str; 5] = ["PATH", "SYSTEMROOT", "TMPDIR", "TEMP", "TMP"];
/// What a reservation of a frame's bytes is called.
const FRAME_RESERVED: &str = "a plug-in's message";

/// How to start a program of an extension.
#[derive(Debug, Clone)]
pub(crate) struct Spawn {
    /// The extension's name, which its WELCOME repeats.
    pub(crate) extension: String,
    /// The executable.
    pub(crate) program: PathBuf,
    /// Its directory, where it runs.
    pub(crate) directory: PathBuf,
    /// The codes of the kinds its manifest declares.
    pub(crate) kinds: Vec<u8>,
    /// The memory budget its frames are reserved from.
    pub(crate) memory: Arc<MemoryBudget>,
}

/// A frame read, its bytes reserved while it is held.
pub(super) struct Frame {
    pub(super) body: Vec<u8>,
    _reserved: Reservation,
}

/// What waiting on a process gave.
pub(super) enum Next {
    /// A message, and the bytes of its frame.
    Message(Message, usize),
    /// Nothing yet.
    Waiting,
}

/// A running program.
pub(super) struct Process {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Result<Frame>>,
    tail: Arc<Mutex<Vec<u8>>>,
    /// What it answered HELLO with.
    pub(super) welcome: Welcome,
}

impl Process {
    /// Starts `spawn`'s program and greets it, bounded by `limits`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T05 if it cannot be started; 38T01, 38T03 or 38T04 if
    /// it does not answer HELLO with a WELCOME of protocol 1 and of its
    /// manifest's name and kinds.
    pub(super) fn start(spawn: &Spawn, limits: &Limits) -> Result<Process> {
        let mut command = Command::new(&spawn.program);
        command.current_dir(&spawn.directory).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (name, value) in KEPT_ENVIRONMENT.iter().filter_map(|n| std::env::var_os(n).map(|v| (n, v))) {
            command.env(name, value);
        }
        let mut child = command.spawn().map_err(|e| unavailable(&format!("{} could not be started: {e}", spawn.program.display())))?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            let _ = child.kill();
            return Err(unavailable("its pipes could not be opened"));
        };
        let (send, frames) = channel();
        let tail = Arc::new(Mutex::new(Vec::new()));
        let started = start_readers(spawn, (stdout, stderr), (send, Arc::clone(&tail)), limits.max_message());
        let welcome = Welcome { protocol: 0, name: String::new(), version: String::new(), capabilities: Vec::new(), kinds: Vec::new() };
        let mut process = Process { child, stdin, frames, tail, welcome };
        started?;
        process.greet(spawn, limits)?;
        Ok(process)
    }

    /// Sends HELLO and takes the WELCOME.
    fn greet(&mut self, spawn: &Spawn, limits: &Limits) -> Result<()> {
        let hello = Hello {
            protocol: super::PROTOCOL,
            extension: spawn.extension.clone(),
            capabilities: HOST_CAPABILITIES.iter().map(|c| (*c).to_owned()).collect(),
            max_message_bytes: limits.max_message(),
            max_call_bytes: limits.max_call(),
            memory_bytes: limits.memory(),
        };
        self.send(&Message::Hello(hello))?;
        let deadline = limits.timeout().and_then(|t| Instant::now().checked_add(t));
        let welcome = loop {
            let wait = deadline.map_or(Duration::MAX, |d| d.saturating_duration_since(Instant::now()));
            if wait.is_zero() {
                return Err(ended("it gave no WELCOME within PLUGINS.CALL_TIMEOUT_MS"));
            }
            match self.next(wait)? {
                Next::Message(Message::Welcome(w), _) => break w,
                Next::Message(other, _) => return Err(violation(&format!("its first message is of type {}, not WELCOME", other.code()))),
                Next::Waiting => {}
            }
        };
        welcomed(&welcome, spawn)?;
        self.welcome = welcome;
        Ok(())
    }

    /// Sends `message`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T01 if its input is closed.
    pub(super) fn send(&mut self, message: &Message) -> Result<()> {
        write_message(&mut self.stdin, message).map_err(|e| self.explained(e))
    }

    /// The next message it sends, waiting at most `wait`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T01 if it ended; 38T03 or 38T04 of what its frame broke.
    pub(super) fn next(&mut self, wait: Duration) -> Result<Next> {
        let frame = match self.frames.recv_timeout(wait) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) => return Ok(Next::Waiting),
            Err(RecvTimeoutError::Disconnected) => Err(ended("its output ended")),
        };
        let frame = frame.map_err(|e| self.explained(e))?;
        let bytes = frame.body.len();
        Ok(Next::Message(Message::decode(&frame.body)?, bytes))
    }

    /// `e`, with the last line its program wrote to its standard error, if
    /// any, after its message.
    fn explained(&self, mut e: crate::error::SqlError) -> crate::error::SqlError {
        let tail = self.tail();
        if let Some(last) = tail.lines().rev().find(|l| !l.trim().is_empty()) {
            e.msg = format!("{}; it wrote: {}", e.msg, last.trim());
        }
        e
    }

    /// What it wrote last to its standard error.
    pub(super) fn tail(&self) -> String {
        String::from_utf8_lossy(&lock_builtin(&self.tail)).into_owned()
    }

    /// Whether it has ended.
    pub(super) fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Asks it to end, waiting `grace` for it to; then stops it.
    pub(super) fn shut_down(mut self, grace: Duration) {
        let _ = self.send(&Message::Shutdown);
        let until = Instant::now().checked_add(grace);
        loop {
            let ended = self.exited();
            if ended || until.is_none_or(|u| Instant::now() >= u) {
                return;
            }
            // Its output's end tells that it ended; a message it sends is
            // of no call and is let go.
            let _ = self.next(Duration::from_millis(super::pool::POLL_MS));
        }
    }
}

/// Stopped and waited for, so it outlives nothing that dropped it.
impl Drop for Process {
    fn drop(&mut self) {
        // A process that ended already cannot be stopped again: either way
        // it is waited for next.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// That `welcome` is protocol 1's, of `spawn`'s extension, serving every
/// kind its manifest declares.
fn welcomed(welcome: &Welcome, spawn: &Spawn) -> Result<()> {
    if welcome.protocol != super::PROTOCOL {
        return Err(violation(&format!("it speaks protocol {}, and the database protocol {}", welcome.protocol, super::PROTOCOL)));
    }
    if welcome.name != spawn.extension {
        return Err(violation(&format!("it names itself {}, and its manifest {}", welcome.name, spawn.extension)));
    }
    if let Some(kind) = spawn.kinds.iter().find(|k| !welcome.kinds.contains(k)) {
        return Err(violation(&format!("its manifest declares kind {kind}, which it does not serve")));
    }
    Ok(())
}

/// Starts the readers of the program's output and error pipes.
fn start_readers(
    spawn: &Spawn,
    (stdout, stderr): (std::process::ChildStdout, std::process::ChildStderr),
    (send, tail): (Sender<Result<Frame>>, Arc<Mutex<Vec<u8>>>),
    max: u64,
) -> Result<()> {
    let memory = Arc::clone(&spawn.memory);
    pipe_reader(&format!("{}-out", spawn.extension), move || read_frames(stdout, &send, (&memory, max)))?;
    pipe_reader(&format!("{}-err", spawn.extension), move || keep_tail(stderr, &tail))
}

/// Hands on each frame `from` holds, its bytes reserved before they are
/// read, until it ends or fails.
fn read_frames(mut from: impl Read, to: &Sender<Result<Frame>>, (memory, max): (&Arc<MemoryBudget>, u64)) {
    loop {
        let frame = read_length(&mut from, max).and_then(|length| {
            let Some(length) = length else { return Err(ended("its output ended")) };
            let reserved = memory.reserve(length, FRAME_RESERVED)?;
            Ok(Frame { body: read_body(&mut from, length)?, _reserved: reserved })
        });
        let failed = frame.is_err();
        if to.send(frame).is_err() || failed {
            return;
        }
    }
}

/// Keeps the last `TAIL_BYTES` `from` holds, until it ends.
fn keep_tail(mut from: impl Read, tail: &Mutex<Vec<u8>>) {
    let mut chunk = [0_u8; ERROR_CHUNK];
    while let Ok(n) = from.read(&mut chunk) {
        let Some(read) = chunk.get(..n).filter(|r| !r.is_empty()) else { return };
        let mut kept = lock_builtin(tail);
        kept.extend_from_slice(read);
        let over = kept.len().saturating_sub(TAIL_BYTES);
        kept.drain(..over);
    }
}
