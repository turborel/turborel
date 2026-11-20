//! \[DESIGN\] A sample of a table's rows (RULES.md D382): up to
//! `SAMPLE_ROWS` whole rows, drawn by Vitter's Algorithm R over the rows
//! a statistics pass reads, every row of a small table and a uniform
//! sample of those a large table's pass samples, from a fixed seed so
//! the same rows give the same sample. The planner evaluates on it a
//! conjunction whose conjuncts go together, or that no column's synopsis
//! answers (`physical/estimate/sampled.rs`): what share of the sample it
//! keeps is the share of the table, whatever the correlation between its
//! columns (Hertzschuch, Hartmann, Habich and Lehner, "Simplicity done
//! right for join ordering", CIDR 2021; "Small selectivities matter",
//! SIGMOD 2021).
//!
//! A character string is kept as its first `ROW_CHARACTERS` characters,
//! a binary string its first `ROW_OCTETS` octets, and a large object as
//! the empty one of its kind, so a sample of long values takes a bounded
//! memory; a pattern is judged on what is kept.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::storage::Row;
use crate::value::Value;
use crate::value::lob::Lob;

use super::parts::{SEED, xorshift};

/// \[DESIGN\] The most rows a sample keeps: enough that a conjunction
/// keeping one row in a hundred is seen ten times, few enough that
/// evaluating a conjunction on them is a small share of planning.
pub const SAMPLE_ROWS: usize = 1_024;
/// \[DESIGN\] The most characters of a string a sampled row keeps:
/// longer than a column's sample keeps (`parts::SAMPLED_CHARACTERS`), as
/// a pattern with a leading wildcard looks inside the value.
pub const ROW_CHARACTERS: usize = 256;
/// \[DESIGN\] The most octets of a binary string a sampled row keeps.
pub const ROW_OCTETS: u32 = 256;

/// \[DESIGN\] The most answers a sample remembers: past them it forgets
/// all and begins again.
pub const REMEMBERED: usize = 1_024;

/// A sample of a table's rows, and the answers asked of it.
#[derive(Debug, Default)]
pub struct RowSample {
    rows: Vec<Row>,
    /// What was found of it, by the question's key (`remembered`).
    answers: Mutex<HashMap<u64, f64>>,
}

impl Clone for RowSample {
    fn clone(&self) -> RowSample {
        RowSample::of_rows(self.rows.clone())
    }
}

impl RowSample {
    /// A sample of `rows`, as an engine that keeps its own gives them.
    pub fn of_rows(rows: Vec<Row>) -> RowSample {
        RowSample { rows, answers: Mutex::default() }
    }

    /// Its rows.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The answer to the question of key `key`, by `find` the first time
    /// it is asked, remembered after: the sample never changes, and the
    /// planner asks the same of it for each plan it weighs and each
    /// statement it plans, where evaluating up to `SAMPLE_ROWS` rows each
    /// time would cost more than the rest of planning. At most
    /// `REMEMBERED` answers are kept.
    pub fn remembered(&self, key: u64, find: impl FnOnce() -> f64) -> f64 {
        if let Some(answer) = self.answers.lock().ok().and_then(|a| a.get(&key).copied()) {
            return answer;
        }
        let answer = find();
        if let Ok(mut answers) = self.answers.lock() {
            if answers.len() >= REMEMBERED {
                answers.clear();
            }
            answers.insert(key, answer);
        }
        answer
    }
}

/// A uniform sample of the rows added, at most `most`.
#[derive(Debug, Clone)]
pub struct RowReservoir {
    rows: Vec<Row>,
    most: usize,
    seen: u64,
    random: u64,
}

impl Default for RowReservoir {
    fn default() -> Self {
        RowReservoir::keeping(SAMPLE_ROWS)
    }
}

impl RowReservoir {
    /// An empty sample of at most `most` rows.
    pub fn keeping(most: usize) -> RowReservoir {
        RowReservoir { rows: Vec::new(), most, seen: 0, random: SEED }
    }

    /// Adds `row`: the n-th replaces a kept one with chance `most` / n,
    /// so every row is kept with the same chance, and is copied only if
    /// it is.
    pub fn add(&mut self, row: &Row) {
        self.seen = self.seen.saturating_add(1);
        if self.rows.len() < self.most {
            self.rows.push(kept(row));
            return;
        }
        let pick = xorshift(&mut self.random).checked_rem(self.seen).unwrap_or_default();
        if let Some(slot) = usize::try_from(pick).ok().and_then(|i| self.rows.get_mut(i)) {
            *slot = kept(row);
        }
    }

    /// The sample, if any row was added.
    pub fn finish(self) -> Option<RowSample> {
        (!self.rows.is_empty()).then(|| RowSample::of_rows(self.rows))
    }
}

/// `row` as a sample keeps it.
fn kept(row: &Row) -> Row {
    row.iter().map(kept_value).collect()
}

/// `value` as a sampled row keeps it: a long string cut, a large object
/// the empty one of its kind, any other whole.
fn kept_value(value: &Value) -> Value {
    match value {
        Value::Char(s) => match s.char_indices().nth(ROW_CHARACTERS) {
            Some((end, _)) => Value::Char(s.get(..end).unwrap_or_default().to_owned()),
            None => value.clone(),
        },
        Value::Binary(b) => {
            if b.len() > ROW_OCTETS {
                Value::Binary(b.substring(1, ROW_OCTETS))
            } else {
                value.clone()
            }
        }
        Value::Lob(l) => Value::Lob(Lob::empty(l.kind())),
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => value.clone(),
    }
}
