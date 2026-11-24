//! \[EXTENSION\] EXPLAIN ANALYZE of a data change statement, INSERT,
//! UPDATE, DELETE or MERGE (RULES.md D622). The statement runs for real,
//! as `Session::execute` runs it, in the session's transaction, so the
//! caller may roll it back. The first plan it makes of the rows it reads
//! is counted as a query's is (`analyzing`): an INSERT's query, the object
//! rows of an UPDATE or DELETE, MERGE's joined rows. Its text is a line
//! naming the statement and its table with the rows it changed, that plan
//! below it, and a last line of those rows. A statement that raises makes
//! EXPLAIN ANALYZE raise the same, its changes undone as any failed
//! statement's are.

use std::fmt::Write;

use super::super::{Session, State};
use super::with_plugin_calls;
use crate::ast::SqlStatement;
use crate::binder::qualify;
use crate::error::Result;
use crate::logical::LogicalPlan;
use crate::params::Params;
use crate::parser::Parser;
use crate::physical::{Analyzed, NotesRead, Operator, PhysicalPlanner, Plan, counted, millis, operators};
use crate::txn::Txn;

/// `plan`, the plan of the rows a data change reads, counted and kept for
/// EXPLAIN ANALYZE if it runs the statement and no plan was counted yet;
/// else `plan` as it is. Counted, it is planned again of `optimized`, its
/// optimized plan, if given, by the planner that describes each operator
/// as EXPLAIN shows it, choosing as the planner that made `plan` chose.
pub(in crate::db) fn analyzing(tx: &Txn, st: &State, (plan, optimized): (Plan, Option<&LogicalPlan>)) -> Plan {
    let changed = tx.stop.changed();
    if !changed.taken() {
        return plan;
    }
    let plan = optimized.map_or(plan, |o| PhysicalPlanner::describing(st).plan(o));
    let (plan, counts) = counted(plan);
    changed.keep((plan.clone(), counts));
    plan
}

impl Session {
    /// \[EXTENSION\] EXPLAIN ANALYZE of `sql`, a data change statement,
    /// its parameters `params`: the statement run in the session's
    /// transaction, and what it and its plan did.
    ///
    /// # Errors
    ///
    /// Any error of parsing or running the statement.
    pub(in crate::db) fn analyzed_change(&self, sql: &str, params: &Params) -> Result<Analyzed> {
        let label = label_of(&Parser::new(sql)?.sql_statement()?, &self.default_schema());
        let st = self.db.snapshot()?;
        let changed = self.stop.changed();
        changed.want(true);
        let began = st.clock.now();
        let ran = self.execute(sql, &mut params.clone());
        let took = u64::try_from(st.clock.now().saturating_sub(began)).unwrap_or_default();
        changed.want(false);
        let kept = changed.kept();
        ran?;
        let (rows, micros) = (u64::try_from(self.row_count()).unwrap_or(u64::MAX), (took > 0).then_some(took));
        let top = Operator { label, depth: 0, estimate: None, rows, loops: 1, micros, tables: 0, join: false, notes: NotesRead::default() };
        let mut text = format!("-- physical:\n{}  (actual rows changed={rows}{})\n", top.label, time_of(micros));
        let mut found = vec![top];
        if let Some((plan, counts)) = kept {
            for count in &counts {
                count.show_time(micros.is_some());
            }
            for line in plan.explain().lines() {
                // Writing to a String cannot fail.
                let _ = writeln!(text, "  {line}");
            }
            let tables = operators(plan, 1, &mut found);
            if let Some(statement) = found.first_mut() {
                statement.tables = tables;
            }
        }
        let plural = if rows == 1 { "" } else { "s" };
        let _ = writeln!(text, "-- ran: {rows} row{plural} changed");
        Ok(Analyzed { operators: found, rows, micros, text: with_plugin_calls(&st, text) })
    }
}

/// `, time=` and `micros` as milliseconds, if known.
fn time_of(micros: Option<u64>) -> String {
    micros.map(|m| format!(", time={}", millis(m))).unwrap_or_default()
}

/// The line naming the data change `stmt` and its table, its unqualified
/// name in `schema`.
pub(super) fn label_of(stmt: &SqlStatement, schema: &str) -> String {
    let kind = if matches!(stmt, SqlStatement::Insert(_)) {
        "Insert into"
    } else if matches!(stmt, SqlStatement::UpdateSearched { .. } | SqlStatement::UpdatePositioned { .. }) {
        "Update"
    } else if matches!(stmt, SqlStatement::DeleteSearched { .. } | SqlStatement::DeletePositioned { .. }) {
        "Delete from"
    } else {
        "Merge into"
    };
    super::super::execute::target_of(stmt).map_or_else(|| kind.to_owned(), |t| format!("{kind} {}", qualify(t, schema)))
}
