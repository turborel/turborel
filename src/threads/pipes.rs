//! \[DESIGN\] The threads that read a plug-in process's pipes (RULES.md
//! D55, D170): one for what it writes to its standard output, and one for
//! its standard error, each named after its extension. A pipe's read
//! blocks with no time limit in std, so a thread reads it and hands each
//! piece on, and the caller waits on what it is handed with a deadline.
//!
//! Each thread ends when its pipe does, which a stopped process's pipe
//! does at once; none is joined, so one blocked by a pipe a process's own
//! child holds open never holds up the database.

use std::thread;

use crate::error::{Code, Result, SqlError};

/// The prefix of a pipe reader's name.
const PREFIX: &str = "turborel-plugin";

/// Starts the thread `name`, after `PREFIX`, running `read` until its
/// pipe ends.
///
/// # Errors
///
/// SQLCODE -915 if the thread cannot be started.
pub(crate) fn pipe_reader(name: &str, read: impl FnOnce() + Send + 'static) -> Result<()> {
    let label = format!("{PREFIX}-{name}");
    thread::Builder::new()
        .name(label.clone())
        .spawn(read)
        .map(drop)
        .map_err(|e| SqlError::new(Code::Io, format!("[DESIGN] the thread {label} could not be started: {e}")))
}
