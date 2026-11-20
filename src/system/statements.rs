//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.STATEMENT_STATISTICS
//! (RULES.md D521): the statements run since the database was opened, by
//! normalized text (`statements/text.rs`) and user, each with its calls,
//! their time, the most one took, its rows, the rows it read and the bytes
//! it spilled, and its last run.
//!
//! Each session tallies its own (`txn/stop/activity/tallies.rs`), so a
//! statement's end takes no lock another session takes. The database
//! lists each session's tallies once it first tallies, and keeps those of
//! sessions ended, merged, until the table is read. STATEMENTS.KEPT bounds
//! each session's and the merged: a new one past them takes the place of
//! the one called least. 0 keeps none, and setting it to 0 forgets every
//! tally, which is how they are reset.
//!
//! The database's owner, and a user granted the role MONITOR the owner
//! created (`Seen::may_monitor`, RULES.md D520), see every text; any other
//! user sees each row, the texts of others' as the null value, as VIEWS
//! hides the definitions of others' views.

mod text;

pub use text::Text;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{Seen, SystemTable, identifier, text, text_column};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::settings::{Configurable, Definition, Effect, Kind, Settings};
use crate::storage::Row;
use crate::sync::lock_builtin;
use crate::txn::{Tallies, Tally};
use crate::value::datetime::Moment;
use crate::value::{DataType, Datetime, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

/// \[DESIGN\] The setting that bounds the tallies, and its group, under
/// which it is declared.
pub const KEPT: &str = "STATEMENTS.KEPT";
/// See [`KEPT`].
pub const GROUP: &str = "STATEMENTS";
/// \[DESIGN\] The default of STATEMENTS.KEPT.
pub const DEFAULT_KEPT: usize = 1_000;
/// \[DESIGN\] The longest text shown, cut there.
pub const TEXT_LENGTH: u32 = 4_096;
/// The digits of the seconds' fraction of LAST_RUN.
const PRECISION: u8 = 6;
/// The hexadecimal digits of a statement's key, its STATEMENT_ID.
const KEY_DIGITS: u32 = 16;

/// The database's statement tallies: those of its sessions, and those of
/// sessions ended.
#[derive(Debug)]
pub struct Statements {
    kept: AtomicUsize,
    live: Mutex<Vec<Arc<Mutex<Tallies>>>>,
    ended: Mutex<Tallies>,
}

impl Default for Statements {
    fn default() -> Self {
        Statements { kept: AtomicUsize::new(DEFAULT_KEPT), live: Mutex::default(), ended: Mutex::default() }
    }
}

impl Statements {
    /// How many tallies each session and the merged keep; 0 for none.
    pub fn kept(&self) -> usize {
        self.kept.load(Ordering::Relaxed)
    }

    /// Lists `tallies`, a session's, to be read with the others.
    pub fn list(&self, tallies: &Arc<Mutex<Tallies>>) {
        let mut live = lock_builtin(&self.live);
        self.retire(&mut live);
        live.push(Arc::clone(tallies));
    }

    /// Merges the tallies of sessions ended, held by `live` alone, into
    /// those kept of ended sessions.
    fn retire(&self, live: &mut Vec<Arc<Mutex<Tallies>>>) {
        let kept = self.kept();
        let mut ended = lock_builtin(&self.ended);
        live.retain(|t| {
            let gone = Arc::strong_count(t) == 1;
            if gone {
                ended.merge(&lock_builtin(t), kept);
            }
            !gone
        });
    }

    /// Every tally, merged.
    pub fn all(&self) -> Tallies {
        let kept = self.kept();
        let mut live = lock_builtin(&self.live);
        self.retire(&mut live);
        let mut all = Tallies::default();
        all.merge(&lock_builtin(&self.ended), kept);
        for tallies in live.iter() {
            all.merge(&lock_builtin(tallies), kept);
        }
        all
    }

    /// Forgets every tally.
    fn clear(&self) {
        lock_builtin(&self.ended).clear();
        for tallies in lock_builtin(&self.live).iter() {
            lock_builtin(tallies).clear();
        }
    }
}

impl Configurable for Statements {
    fn settings(&self) -> Vec<Definition> {
        vec![Definition {
            name: KEPT.to_owned(),
            kind: Kind::Integer { min: 0, max: i128::from(u32::MAX) },
            default: Value::Exact(Decimal::new(i128::try_from(DEFAULT_KEPT).unwrap_or(i128::MAX), 0)),
            per_session: false,
            node: false,
            effect: Effect::Statement,
            doc: "The most statements, by normalized text and user, INFORMATION_SCHEMA.STATEMENT_STATISTICS keeps, each session's and those of sessions ended alike, past which a new one takes the place of the one called least; 0 keeps none, and setting it to 0 forgets them all.".to_owned(),
        }]
    }

    fn configure(&self, settings: &Settings) {
        let Some(key) = settings.key(KEPT) else { return };
        let kept = settings.count(key);
        // Told after every change of any setting: only a change of this
        // one to 0 forgets them.
        let before = self.kept.swap(kept, Ordering::Relaxed);
        if kept == 0 && before != 0 {
            self.clear();
        }
    }
}

/// \[EXTENSION\] INFORMATION_SCHEMA.STATEMENT_STATISTICS.
pub(super) struct StatementStatistics;

/// A column of counts, named `name`.
fn counted(name: &str) -> ColumnDesc {
    ColumnDesc {
        data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
        ..identifier(name, true)
    }
}

/// A count's value.
fn count(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The text `s`, cut at `TEXT_LENGTH` characters.
pub(crate) fn cut(s: &str) -> Value {
    text(&s.chars().take(usize::try_from(TEXT_LENGTH).unwrap_or(usize::MAX)).collect::<String>())
}

/// The value of a time, a timestamp's instant `utc`.
pub(crate) fn time(utc: i64) -> Value {
    Value::Datetime(Datetime { moment: Moment::Timestamp, utc, zone: 0, zoned: true, precision: PRECISION })
}

/// A column of a time, named `name`, which may be null.
pub(crate) fn time_column(name: &str) -> ColumnDesc {
    ColumnDesc { data_type: DataType::Timestamp { precision: u32::from(PRECISION), zoned: true }, ..identifier(name, false) }
}

/// The row of `t`, its text shown if `shown`.
fn row(t: &Tally, shown: bool) -> Row {
    let mean = t.micros.checked_div(t.calls).unwrap_or(0);
    vec![
        text(&t.user),
        text(&format!("{:016x}", t.text.key())),
        if shown { cut(t.text.normalized()) } else { Value::Null },
        count(t.calls),
        count(t.micros),
        count(mean),
        count(t.most),
        count(t.rows),
        count(t.read),
        count(t.spilled),
        time(t.last),
    ]
}

impl SystemTable for StatementStatistics {
    fn name(&self) -> &'static str {
        "STATEMENT_STATISTICS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut columns = vec![
            identifier("USER_NAME", true),
            text_column(("STATEMENT_ID", KEY_DIGITS), true),
            text_column(("STATEMENT_TEXT", TEXT_LENGTH), false),
        ];
        let counts = ["CALLS", "TOTAL_MICROS", "MEAN_MICROS", "MAX_MICROS", "ROW_COUNT", "ROWS_READ", "BYTES_SPILLED"];
        columns.extend(counts.into_iter().map(counted));
        columns.push(time_column("LAST_RUN"));
        columns
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let all = seen.state.ran.statements.all();
        let every = seen.may_monitor();
        let mut rows: Vec<(u64, &str, Row)> =
            all.iter().map(|t| (t.text.key(), &*t.user, row(t, every || &*t.user == seen.auth))).collect();
        rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        Ok(rows.into_iter().map(|(_, _, r)| r).collect())
    }
}
