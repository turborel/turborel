//! \[DESIGN\] RULES.md D36: the values the settings taken as what reads
//! them starts (`Effect::Start`) had as it started, so a change that waits
//! for a restart says so where settings are shown. Split from settings.rs
//! (RULES.md R-SIZE-2).

use std::sync::Mutex;

use crate::sync::lock_builtin;
use crate::value::Value;

/// The values settings had as what reads them started, by name: shared by
/// a database's snapshots of its definitions, as what started holds them
/// whatever the definitions become; never kept, as nothing started is once
/// the database is closed.
#[derive(Debug, Default)]
pub struct Started(Mutex<Vec<(String, Value)>>);

impl Started {
    /// Records `value` as the one the setting `name` was started with,
    /// over any recorded before.
    pub fn record(&self, name: &str, value: Value) {
        let mut all = lock_builtin(&self.0);
        all.retain(|(n, _)| n != name);
        all.push((name.to_owned(), value));
    }

    /// The value the setting `name` was started with, if what reads it
    /// started.
    pub fn get(&self, name: &str) -> Option<Value> {
        lock_builtin(&self.0).iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }
}
