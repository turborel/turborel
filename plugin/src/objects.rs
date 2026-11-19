//! \[EXTENSION\] What a plug-in's calls keep between them (RULES.md D167):
//! its exchanges, targets and uploads, each by the number it gave the
//! database, and the rows of a system table being read a page at a time.

use std::collections::HashMap;

use turborel::value::Value;

use super::kinds::{Exchange, TargetStore, Upload};
use super::{Answer, PluginError};

/// What calls keep.
#[derive(Default)]
pub(super) struct Held {
    next: u64,
    pub(super) exchanges: HashMap<u64, Box<dyn Exchange>>,
    pub(super) stores: HashMap<u64, Box<dyn TargetStore>>,
    pub(super) uploads: HashMap<u64, Box<dyn Upload>>,
    /// The rows of a system table, by its name and its reader, being read.
    pub(super) pages: HashMap<(String, String, bool), Vec<Vec<Value>>>,
}

impl Held {
    /// A number for a new exchange, target or upload.
    pub(super) fn number(&mut self) -> u64 {
        self.next = self.next.saturating_add(1);
        self.next
    }
}

/// The entry of `n` in `all`.
///
/// # Errors
///
/// SQLSTATE 38000 if there is none.
pub(super) fn numbered<'a, T: ?Sized>(all: &'a mut HashMap<u64, Box<T>>, n: u64, what: &str) -> Answer<&'a mut Box<T>> {
    all.get_mut(&n).ok_or_else(|| PluginError::external(&format!("no {what} has number {n}")))
}
