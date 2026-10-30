//! \[DESIGN\] What the server shows of itself (RULES.md D42, D55): two
//! system tables it registers with the database it serves, beside the
//! database's own, its rows the database's owner's alone.
//!
//! * INFORMATION_SCHEMA.CONNECTIONS: each connection served, by its
//!   number: its user once authenticated, where it came from, whether it
//!   is secure, and whether it is logging in, idle or running a
//!   statement.
//! * INFORMATION_SCHEMA.SERVER: one row of counts: workers running a
//!   statement, statements waiting for one, connections, and those refused
//!   past SERVER.MAX_CONNECTIONS, told or closed at once.
//!
//! A server not serving shows no connections, and counts of none.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use turborel::catalog::ColumnDesc;
use turborel::error::Result;
use turborel::storage::Row;
use turborel::system::{Seen, SystemTable};
use turborel::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

use crate::auth::Peer;

/// What the server shows: its connections, and its counts.
#[derive(Default)]
pub(crate) struct Shown {
    next: AtomicU64,
    connections: Mutex<BTreeMap<u64, Entry>>,
    /// Workers running a statement.
    pub(crate) busy: AtomicU64,
    /// Statements waiting for a worker.
    pub(crate) waiting: AtomicU64,
    /// Connections refused past SERVER.MAX_CONNECTIONS, told why.
    pub(crate) refused: AtomicU64,
    /// Connections closed at once past the refusals in flight.
    pub(crate) closed: AtomicU64,
}

/// A connection, as CONNECTIONS shows it.
#[derive(Clone)]
struct Entry {
    user: Option<String>,
    peer: String,
    secure: bool,
    state: &'static str,
}

/// \[DESIGN\] A connection's row of CONNECTIONS, while it is served: what
/// serves it says who logged in and whether a statement runs; dropped, the
/// row goes.
pub struct Present {
    shown: Arc<Shown>,
    id: u64,
}

impl Shown {
    /// The row of a connection taken from `peer`, logging in.
    pub(crate) fn connected(self: &Arc<Self>, peer: &Peer) -> Present {
        let id = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let entry = Entry { user: None, peer: peer.name(), secure: peer.secure, state: "LOGIN" };
        self.locked().insert(id, entry);
        Present { shown: Arc::clone(self), id }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Entry>> {
        // Each row is replaced whole, so a lock a panic poisoned is whole.
        self.connections.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Present {
    /// Its user, `user`, authenticated: idle from now.
    pub fn authenticated(&self, user: &str) {
        if let Some(entry) = self.shown.locked().get_mut(&self.id) {
            entry.user = Some(user.to_owned());
            entry.state = "IDLE";
        }
    }

    /// Whether a statement of it runs now.
    pub fn running(&self, running: bool) {
        if let Some(entry) = self.shown.locked().get_mut(&self.id) {
            entry.state = if running { "RUNNING" } else { "IDLE" };
        }
    }
}

impl Drop for Present {
    fn drop(&mut self) {
        self.shown.locked().remove(&self.id);
    }
}

/// A count, as a column holds it.
fn count(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A column of counts, named `name`.
fn counted(name: &str) -> ColumnDesc {
    let data_type = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };
    ColumnDesc { name: name.to_owned(), data_type, not_null: true, default: None, domain: None }
}

/// A column of text of at most `length` characters, named `name`.
fn text(name: &str, length: u32, not_null: bool) -> ColumnDesc {
    ColumnDesc { name: name.to_owned(), data_type: DataType::Varying { length }, not_null, default: None, domain: None }
}

/// \[EXTENSION\] INFORMATION_SCHEMA.CONNECTIONS.
pub(crate) struct Connections(pub(crate) Arc<Shown>);

impl SystemTable for Connections {
    fn name(&self) -> &'static str {
        "CONNECTIONS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![counted("ID"), text("USER_NAME", 128, false), text("PEER", 128, true), text("SECURE", 3, true), text("STATE", 7, true)]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let yes = |is: bool| Value::Char(if is { "YES" } else { "NO" }.to_owned());
        let rows = self.0.locked().clone();
        Ok(rows
            .into_iter()
            .map(|(id, e)| {
                vec![
                    count(id),
                    e.user.map_or(Value::Null, Value::Char),
                    Value::Char(e.peer),
                    yes(e.secure),
                    Value::Char(e.state.to_owned()),
                ]
            })
            .collect())
    }
}

/// \[EXTENSION\] INFORMATION_SCHEMA.SERVER.
pub(crate) struct Server(pub(crate) Arc<Shown>);

impl SystemTable for Server {
    fn name(&self) -> &'static str {
        "SERVER"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        ["WORKERS_BUSY", "STATEMENTS_WAITING", "CONNECTIONS", "REFUSED", "CLOSED_UNTOLD"].into_iter().map(counted).collect()
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let shown = &self.0;
        let connections = u64::try_from(shown.locked().len()).unwrap_or(u64::MAX);
        let read = |n: &AtomicU64| count(n.load(Ordering::Relaxed));
        Ok(vec![vec![read(&shown.busy), read(&shown.waiting), count(connections), read(&shown.refused), read(&shown.closed)]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D55: a connection taken is shown
    /// logging in, from where it came, a Unix socket's as local; idle once
    /// authenticated, running as a statement runs; and its row goes as it
    /// ends.
    #[test]
    fn a_connection_is_shown_while_it_is_served() {
        let shown = Arc::new(Shown::default());
        let present = shown.connected(&Peer::local());
        let row = |of: &Shown| of.locked().values().next().map(|e| (e.user.clone(), e.peer.clone(), e.secure, e.state));
        assert_eq!(row(&shown), Some((None, "local".to_owned(), true, "LOGIN")));
        present.authenticated("ANN");
        assert_eq!(row(&shown).map(|r| (r.0, r.3)), Some((Some("ANN".to_owned()), "IDLE")));
        present.running(true);
        assert_eq!(row(&shown).map(|r| r.3), Some("RUNNING"));
        present.running(false);
        assert_eq!(row(&shown).map(|r| r.3), Some("IDLE"));
        drop(present);
        assert!(shown.locked().is_empty(), "gone");
    }
}
