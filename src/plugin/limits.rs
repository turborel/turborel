//! \[DESIGN\] What bounds the plug-in processes of a database (RULES.md
//! D172, D173): the settings PLUGINS.*, as the database tells them
//! (`Database::apply_settings`), read by each pool as a call begins, so a
//! change takes effect from the next call.
//!
//! Each frame a program sends is bounded, and so is all it sends in one
//! call. The memory a program uses is told to it in HELLO, as capping a
//! process's own memory needs calls of the operating system the project
//! does not make (D172).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::error::Result;
use crate::sync::lock_builtin;

use super::errors::unavailable;

/// \[DESIGN\] The default of PLUGINS.PROCESSES: the processes of one
/// extension at once, enough for a few statements calling it together.
pub const DEFAULT_PROCESSES: u64 = 4;
/// \[DESIGN\] The default of PLUGINS.CALL_TIMEOUT_MS: half a minute, long
/// for a function and enough for a model's batch.
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;
/// \[DESIGN\] The default of PLUGINS.MAX_MESSAGE_BYTES: 16 MiB.
pub const DEFAULT_MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
/// \[DESIGN\] The default of PLUGINS.MAX_CALL_BYTES: 64 MiB.
pub const DEFAULT_MAX_CALL_BYTES: u64 = 64 * 1024 * 1024;
/// \[DESIGN\] The default of PLUGINS.MEMORY_BYTES: 256 MiB.
pub const DEFAULT_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
/// \[DESIGN\] The default of PLUGINS.CANCEL_GRACE_MS: a second for a
/// program to answer CANCEL before it is stopped.
pub const DEFAULT_CANCEL_GRACE_MS: u64 = 1_000;
/// \[DESIGN\] What separates the directories of PLUGINS.DIRECTORIES: a
/// character no path on the three platforms needs.
pub const DIRECTORY_SEPARATOR: char = ';';

/// The limits, as the settings were last told.
#[derive(Debug)]
pub struct Limits {
    processes: AtomicU64,
    timeout_ms: AtomicU64,
    max_message: AtomicU64,
    max_call: AtomicU64,
    memory: AtomicU64,
    grace_ms: AtomicU64,
    directories: Mutex<Directories>,
}

/// The values of the settings PLUGINS.*, as told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    /// PLUGINS.PROCESSES.
    pub processes: u64,
    /// PLUGINS.CALL_TIMEOUT_MS.
    pub timeout_ms: u64,
    /// PLUGINS.MAX_MESSAGE_BYTES.
    pub max_message: u64,
    /// PLUGINS.MAX_CALL_BYTES.
    pub max_call: u64,
    /// PLUGINS.MEMORY_BYTES.
    pub memory: u64,
    /// PLUGINS.CANCEL_GRACE_MS.
    pub grace_ms: u64,
    /// PLUGINS.DIRECTORIES.
    pub directories: String,
}

impl Default for Told {
    fn default() -> Told {
        Told {
            processes: DEFAULT_PROCESSES,
            timeout_ms: DEFAULT_CALL_TIMEOUT_MS,
            max_message: DEFAULT_MAX_MESSAGE_BYTES,
            max_call: DEFAULT_MAX_CALL_BYTES,
            memory: DEFAULT_MEMORY_BYTES,
            grace_ms: DEFAULT_CANCEL_GRACE_MS,
            directories: String::new(),
        }
    }
}

impl Default for Limits {
    fn default() -> Limits {
        let limits = Limits {
            processes: AtomicU64::new(0),
            timeout_ms: AtomicU64::new(0),
            max_message: AtomicU64::new(0),
            max_call: AtomicU64::new(0),
            memory: AtomicU64::new(0),
            grace_ms: AtomicU64::new(0),
            directories: Mutex::new(Directories::default()),
        };
        limits.tell(&Told::default());
        limits
    }
}

impl Limits {
    /// Takes `told`, the settings' values now.
    pub fn tell(&self, told: &Told) {
        let pairs = [
            (&self.processes, told.processes.max(1)),
            (&self.timeout_ms, told.timeout_ms),
            (&self.max_message, told.max_message),
            (&self.max_call, told.max_call),
            (&self.memory, told.memory),
            (&self.grace_ms, told.grace_ms),
        ];
        for (slot, value) in pairs {
            slot.store(value, Ordering::Relaxed);
        }
        *lock_builtin(&self.directories) = Directories::parse(&told.directories);
    }

    /// The most processes of an extension at once: at least 1.
    pub fn processes(&self) -> usize {
        usize::try_from(self.processes.load(Ordering::Relaxed)).unwrap_or(usize::MAX)
    }

    /// The time a call may take, if bounded.
    pub fn timeout(&self) -> Option<Duration> {
        let ms = self.timeout_ms.load(Ordering::Relaxed);
        (ms > 0).then(|| Duration::from_millis(ms))
    }

    /// The milliseconds of `timeout`, 0 for none.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms.load(Ordering::Relaxed)
    }

    /// The most bytes of one frame.
    pub fn max_message(&self) -> u64 {
        self.max_message.load(Ordering::Relaxed)
    }

    /// The most bytes a program sends in one call.
    pub fn max_call(&self) -> u64 {
        self.max_call.load(Ordering::Relaxed)
    }

    /// The memory told to a program.
    pub fn memory(&self) -> u64 {
        self.memory.load(Ordering::Relaxed)
    }

    /// How long a program has to answer CANCEL.
    pub fn grace(&self) -> Duration {
        Duration::from_millis(self.grace_ms.load(Ordering::Relaxed))
    }

    /// The directories allowed.
    pub fn directories(&self) -> Directories {
        lock_builtin(&self.directories).clone()
    }
}

/// \[DESIGN\] The directories plug-ins may be in: PLUGINS.DIRECTORIES's,
/// each a directory under which any plug-in directory may sit (RULES.md
/// D173).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directories(Vec<PathBuf>);

impl Directories {
    /// The directories `text` names, separated by `DIRECTORY_SEPARATOR`,
    /// those empty passed over.
    pub fn parse(text: &str) -> Directories {
        Directories(text.split(DIRECTORY_SEPARATOR).map(str::trim).filter(|d| !d.is_empty()).map(PathBuf::from).collect())
    }

    /// The directories, in the order written: the first is where a
    /// package is installed (design document §13.3).
    pub fn paths(&self) -> &[PathBuf] {
        &self.0
    }

    /// `directory`, made absolute and its links followed, if it is under
    /// one of these, so followed too.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T05 if it cannot be found, or none of these holds it.
    pub fn allowed(&self, directory: &Path) -> Result<PathBuf> {
        let found = std::fs::canonicalize(directory)
            .map_err(|e| unavailable(&format!("its directory {} cannot be found: {e}", directory.display())))?;
        let held = self.0.iter().filter_map(|d| std::fs::canonicalize(d).ok()).any(|d| found.starts_with(d));
        if held {
            return Ok(found);
        }
        Err(unavailable(&format!(
            "its directory {} is under none of PLUGINS.DIRECTORIES, which the host sets as the database is opened",
            found.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D173: PLUGINS.DIRECTORIES is split at
    /// `;`, each part trimmed and an empty one passed over; nothing is
    /// allowed while it is empty; a directory under one is allowed, made
    /// absolute, and one beside it, or one missing, is refused with
    /// SQLSTATE 38T05.
    #[test]
    fn a_directory_is_allowed_only_under_one_of_the_settings() {
        assert_eq!(Directories::parse(" a ; ;b;").0, [PathBuf::from("a"), PathBuf::from("b")]);
        let root = std::env::temp_dir().join(format!("turborel-limits-{}", std::process::id()));
        let (inside, beside) = (root.join("allowed").join("ext"), root.join("other"));
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&beside).unwrap();
        let none = Directories::default();
        assert_eq!(none.allowed(&inside).unwrap_err().sqlstate(), super::super::errors::UNAVAILABLE);
        let some = Directories::parse(&root.join("allowed").display().to_string());
        assert_eq!(some.allowed(&inside).unwrap(), std::fs::canonicalize(&inside).unwrap());
        assert!(some.allowed(&beside).is_err());
        assert!(some.allowed(&root.join("missing")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Verifies: [DESIGN] RULES.md D172: the limits take the defaults
    /// until told; PLUGINS.PROCESSES of 0 is 1, and a timeout of 0 is
    /// none.
    #[test]
    fn the_limits_are_the_settings_told() {
        let limits = Limits::default();
        assert_eq!((limits.processes(), limits.timeout()), (4, Some(Duration::from_millis(DEFAULT_CALL_TIMEOUT_MS))));
        limits.tell(&Told { processes: 0, timeout_ms: 0, ..Told::default() });
        assert_eq!((limits.processes(), limits.timeout()), (1, None));
        assert_eq!(limits.max_message(), DEFAULT_MAX_MESSAGE_BYTES);
    }
}
