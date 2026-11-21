//! \[DESIGN\] What CONNECTIONS shows of what each connection's session does
//! now (RULES.md D520), read from its activity (`turborel::txn::Activity`):
//! the text of the statement it runs and when it began, when its open
//! transaction began, and what it waits on. A lock is shown with the
//! connection whose session's transaction holds it; a statement waiting for
//! a worker is the server's own to say.

use std::collections::HashMap;

use turborel::catalog::ColumnDesc;
use turborel::txn::{Stop, TxId, Waiting};
use turborel::value::datetime::Moment;
use turborel::value::{DataType, Datetime, Value};

use super::{count, counted_or_null, text};

/// \[DESIGN\] The longest statement text or lock name shown, cut there.
pub(crate) const TEXT_LENGTH: u32 = 4_096;
/// The digits of the seconds' fraction of a time shown.
const PRECISION: u8 = 6;
/// The longest WAITING_ON, WORKER.
const WAITING_LENGTH: u32 = 6;
/// WAITING_ON of a lock.
pub(crate) const LOCK: &str = "LOCK";
/// WAITING_ON of the log's sync.
pub(crate) const IO: &str = "IO";
/// WAITING_ON of a statement waiting for a worker.
pub(crate) const WORKER: &str = "WORKER";

/// A column of a time, which may be null.
fn time_column(name: &str) -> ColumnDesc {
    ColumnDesc { data_type: DataType::Timestamp { precision: u32::from(PRECISION), zoned: true }, ..text(name, 1, false) }
}

/// The columns of what a session does now, after the others.
pub(crate) fn columns() -> Vec<ColumnDesc> {
    vec![
        text("STATEMENT_TEXT", TEXT_LENGTH, false),
        time_column("STATEMENT_BEGAN"),
        time_column("TRANSACTION_BEGAN"),
        text("WAITING_ON", WAITING_LENGTH, false),
        text("LOCK_NAME", TEXT_LENGTH, false),
        counted_or_null("LOCK_HOLDER"),
    ]
}

/// The value of a time, a timestamp's instant `utc`.
fn time(utc: i64) -> Value {
    Value::Datetime(Datetime { moment: Moment::Timestamp, utc, zone: 0, zoned: true, precision: PRECISION })
}

/// `s`, cut at `TEXT_LENGTH` characters.
fn cut(s: &str) -> Value {
    Value::Char(s.chars().take(usize::try_from(TEXT_LENGTH).unwrap_or(usize::MAX)).collect())
}

/// The connection of each open transaction, of the sessions `stops` gives
/// with their connections' IDs.
pub(crate) fn holders<'a>(stops: impl Iterator<Item = (u64, &'a Stop)>) -> HashMap<TxId, u64> {
    stops.filter_map(|(id, stop)| stop.activity().now().transaction.map(|(tx, _)| (tx, id))).collect()
}

/// What a connection is doing, as CONNECTIONS shows it.
pub(crate) struct Doing<'a> {
    /// Its session's cancel flag, which holds its activity, once it has one.
    pub(crate) stop: Option<&'a Stop>,
    /// Whether a statement of it runs now.
    pub(crate) running: bool,
    /// Whether a request of it waits for a worker.
    pub(crate) queued: bool,
}

/// The values of the columns `columns` names, of the connection `doing`,
/// the holder of a lock found among `holders`.
pub(crate) fn values(doing: &Doing<'_>, holders: &HashMap<TxId, u64>) -> Vec<Value> {
    let now = doing.stop.map(|s| s.activity().now());
    let statement = now.as_ref().and_then(|n| n.statement.as_ref()).filter(|_| doing.running);
    let transaction = now.as_ref().and_then(|n| n.transaction).map_or(Value::Null, |(_, began)| time(began));
    let (waiting, name, holder) = match now.as_ref().and_then(|n| n.waiting.clone()) {
        Some(Waiting::Lock { what, holder }) => {
            (Value::Char(LOCK.to_owned()), cut(&what), holders.get(&holder).map_or(Value::Null, |&id| count(id)))
        }
        Some(Waiting::Io) => (Value::Char(IO.to_owned()), Value::Null, Value::Null),
        None => (if doing.queued { Value::Char(WORKER.to_owned()) } else { Value::Null }, Value::Null, Value::Null),
    };
    vec![
        statement.map_or(Value::Null, |(t, _)| cut(t.raw())),
        statement.map_or(Value::Null, |&(_, began)| time(began)),
        transaction,
        waiting,
        name,
        holder,
    ]
}
