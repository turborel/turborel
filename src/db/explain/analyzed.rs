//! \[EXTENSION\] EXPLAIN ANALYZE (RULES.md D376): a `<cursor specification>`
//! planned as EXPLAIN plans it, its physical plan counted
//! (`physical/analyzed.rs`), then run in the session's transaction as a
//! cursor of it would be, under the same locks, every row read and
//! dropped, so it changes nothing a query does not. Its text is the
//! physical plan, each operator's line followed by what it did, and a
//! last line of the query's rows and time. A query that raises an
//! exception makes EXPLAIN ANALYZE raise it, as reading its rows would.
//! A program reaches the same by `Session::explain_analyze`, or each
//! operator's count by `Session::analyzed`.

use std::fmt::Write;
use std::sync::Arc;

use super::super::area::commands;
use super::super::{Cursor, Session, State, api};
use super::{plan_rows, with_plugin_calls};
use crate::error::Result;
use crate::exec::{execute, storage_counted};
use crate::params::Params;
use crate::physical::{Analyzed, Plan, counted, millis, operators};
use crate::txn::Txn;

impl Session {
    /// \[EXTENSION\] EXPLAIN ANALYZE of `sql`, a `<cursor specification>`,
    /// its parameters the values in `params`: the query run in the
    /// session's transaction, every row read and dropped, and the text
    /// EXPLAIN ANALYZE shows of it.
    ///
    /// # Errors
    ///
    /// Any error of parsing, binding or running the query.
    pub fn explain_analyze(&self, sql: &str, params: &Params) -> Result<String> {
        Ok(self.analyzed(sql, params)?.text)
    }

    /// \[EXTENSION\] As `explain_analyze`, what each operator did, for a
    /// tool to read: its line, depth, estimate, rows, openings and time.
    ///
    /// # Errors
    ///
    /// As `explain_analyze`.
    pub fn analyzed(&self, sql: &str, params: &Params) -> Result<Analyzed> {
        let mut made = None;
        drop(self.analyzed_cursor(sql, params, &mut made)?);
        made.ok_or_else(|| api("[DESIGN] EXPLAIN ANALYZE ran no query"))
    }

    /// \[EXTENSION\] `EXPLAIN ANALYZE <cursor specification>`, `sql` the
    /// specification: a cursor of one column, PLAN, whose rows are the
    /// lines `explain_analyze` gives, in order.
    ///
    /// # Errors
    ///
    /// As `explain_analyze`.
    pub(in crate::db) fn analyze_rows(&self, sql: &str) -> Result<Cursor> {
        self.analyzed_cursor(sql, &Params::new(), &mut None)
    }

    /// The cursor of the lines of EXPLAIN ANALYZE of `sql`, what it found
    /// put in `made`.
    fn analyzed_cursor(&self, sql: &str, params: &Params, made: &mut Option<Analyzed>) -> Result<Cursor> {
        self.open_with((commands::EXPLAIN, commands::UNRECOGNIZED), |call| {
            self.unnamed(call, None, |st, tx| {
                let analyzed = self.run_analyzed(st, tx, (sql, params))?;
                let rows = plan_rows(st, &analyzed.text)?;
                *made = Some(analyzed);
                Ok(rows)
            })
        })
    }

    /// `sql`, with `params`, planned, counted and run in `tx`.
    fn run_analyzed(&self, st: &Arc<State>, tx: &Txn, (sql, params): (&str, &Params)) -> Result<Analyzed> {
        let spec = Self::parse_cursor(st, sql, params)?;
        let compiled = self.compile(st, &spec, params)?;
        // [X3.135-1986 §8.8 GR3b] the locks a cursor of it takes as it opens.
        for table in compiled.optimized.tables() {
            st.read_lock(tx, &table)?;
        }
        let by_storage = storage_counted(&compiled.physical, st).map(ToString::to_string);
        let (plan, counts) = counted(compiled.physical);
        let shown = plan.clone();
        let (rows, micros) = ran(plan, st, tx)?;
        for count in &counts {
            count.show_time(micros.is_some());
        }
        let text = text_of(st, &shown, by_storage.as_deref(), (rows, micros));
        let mut found = Vec::new();
        operators(shown, 0, &mut found);
        Ok(Analyzed { operators: found, rows, micros, text })
    }
}

/// Runs `plan` in `tx`, reading every row: how many, and the microseconds
/// it took by `st`'s clock, if it moved.
///
/// # Errors
///
/// The first error reading a row raises.
fn ran(plan: Plan, st: &Arc<State>, tx: &Txn) -> Result<(u64, Option<u64>)> {
    let began = st.clock.now();
    let mut rows = 0_u64;
    for read in execute(plan, st, tx.id) {
        read?;
        rows = rows.saturating_add(1);
    }
    let took = u64::try_from(st.clock.now().saturating_sub(began)).unwrap_or_default();
    Ok((rows, (took > 0).then_some(took)))
}

/// The text EXPLAIN ANALYZE shows of `shown`, a counted plan run, which
/// gave `rows` rows in `micros`: the plan, the table its storage counted,
/// if one, and the rows and time.
fn text_of(st: &State, shown: &Plan, by_storage: Option<&str>, (rows, micros): (u64, Option<u64>)) -> String {
    let mut text = format!("-- physical:\n{}", shown.explain());
    // Writing to a String cannot fail.
    if let Some(table) = by_storage {
        let _ = writeln!(text, "-- counted by storage: {table}");
    }
    let plural = if rows == 1 { "" } else { "s" };
    let time = micros.map(|m| format!(" in {}", millis(m))).unwrap_or_default();
    let _ = writeln!(text, "-- ran: {rows} row{plural}{time}");
    with_plugin_calls(st, text)
}
