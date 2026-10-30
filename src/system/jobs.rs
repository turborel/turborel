//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.JOBS: each
//! job of the database's background work (RULES.md D55), an object
//! "associated with INFORMATION_SCHEMA" this implementation adds: its
//! label, whether it is idle, due or running, whether a thread of its own
//! runs it, how often it ran and panicked, and its last error. The
//! database's owner sees them; any other, none, as MEMORY_RESERVATIONS.
//! Split from system.rs (RULES.md R-SIZE-2).

use super::{Seen, SystemTable, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;
use crate::value::Spelled;
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Value};

/// \[DESIGN\] The longest label shown.
const LABEL_LENGTH: u32 = 128;
/// \[DESIGN\] The longest error shown, cut there.
const ERROR_LENGTH: u32 = 1024;

/// \[EXTENSION\] INFORMATION_SCHEMA.JOBS.
pub(super) struct Jobs;

impl SystemTable for Jobs {
    fn name(&self) -> &'static str {
        "JOBS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let count = |name: &str| ColumnDesc {
            name: name.to_owned(),
            data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
            not_null: true,
            default: None,
            domain: None,
        };
        vec![
            text_column(("LABEL", LABEL_LENGTH), true),
            text_column(("STATE", 7), true),
            text_column(("THREADED", 3), true),
            count("RUNS"),
            count("RESTARTS"),
            text_column(("LAST_ERROR", ERROR_LENGTH), false),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if seen.state.users.owner() != Some(seen.auth) {
            return Ok(Vec::new());
        }
        let count = |n: u64| Value::Exact(Decimal::new(i128::from(n), 0));
        let cut = |s: String| text(&s.chars().take(usize::try_from(ERROR_LENGTH).unwrap_or(usize::MAX)).collect::<String>());
        Ok(seen
            .state
            .jobs
            .infos()
            .into_iter()
            .map(|t| {
                vec![
                    text(t.label),
                    text(t.state.word()),
                    yes(t.threaded),
                    count(t.runs),
                    count(t.restarts),
                    t.last_error.map_or(Value::Null, |e| cut(e.to_string())),
                ]
            })
            .collect())
    }
}
