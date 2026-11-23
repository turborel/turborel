//! \[DESIGN\] Showing how a query is planned: its logical plan, the rewrite
//! rules applied, and the physical plan. A program asks for it by
//! `Session::explain`; \[EXTENSION\] direct SQL by `EXPLAIN <cursor
//! specification>`, whose rows are the lines of the same text. EXPLAIN
//! ANALYZE runs the query and shows what each operator did
//! (`explain/analyzed.rs`).

mod analyzed;
mod changed;
mod injected;

use std::sync::Arc;

use super::area::commands;
use super::compiled::Compiled;
use super::stream::Stream;
use super::{Cursor, Session, State};
use crate::binder::OutColumn;
use crate::error::Result;
use crate::params::Params;
use crate::physical::Plan;
use crate::value::{DataType, Value};
pub(super) use changed::analyzing;

/// The name of EXPLAIN's one column.
const PLAN_COLUMN: &str = "PLAN";

impl Session {
    /// Every stage for a `<cursor specification>`: the plan as bound, the rules
    /// that fired, the optimized plan, and the physical plan. Its parameters
    /// have the values in `params`, as at OPEN [X3.135-1986 §8.8 GR3a].
    /// Maintenance its planning noted due is asked for, as no transaction's
    /// end asks for it here (RULES.md D473).
    ///
    /// # Errors
    ///
    /// Any error of parsing or binding the cursor specification.
    pub fn explain(&self, sql: &str, params: &Params) -> Result<String> {
        let call = self.enter()?;
        let planned = self.planned(call.state(), sql, params);
        self.db.maintain_if_due();
        planned
    }

    /// Just the physical plan, its parameters with the values in `params`;
    /// maintenance its planning noted due asked for, as `explain` asks.
    ///
    /// # Errors
    ///
    /// Any error of parsing or binding the cursor specification.
    pub fn physical_plan(&self, sql: &str, params: &Params) -> Result<String> {
        let call = self.enter()?;
        let st = call.state();
        let spec = Self::parse_cursor(st, sql, params)?;
        let planned = self.compile(st, &spec, params).map(|compiled| compiled.physical.explain());
        self.db.maintain_if_due();
        planned
    }

    /// The state `sql` is planned in and its optimized logical plan, for
    /// the planner's unit tests.
    #[cfg(test)]
    pub(crate) fn optimized(&self, sql: &str) -> Result<(Arc<State>, crate::logical::LogicalPlan)> {
        let call = self.enter()?;
        let st = Arc::clone(call.state());
        let spec = Self::parse_cursor(&st, sql, &Params::new())?;
        let optimized = self.compile(&st, &spec, &Params::new())?.optimized;
        Ok((st, optimized))
    }

    /// The state `sql` is bound in and its logical plan as bound, before
    /// any rule, for the optimizer's unit tests.
    #[cfg(test)]
    pub(crate) fn bound_plan(&self, sql: &str) -> Result<(Arc<State>, crate::logical::LogicalPlan)> {
        let call = self.enter()?;
        let st = Arc::clone(call.state());
        let spec = Self::parse_cursor(&st, sql, &Params::new())?;
        let (_, logical) = self.cursor_bound(&st, &spec, &Params::new(), None)?;
        Ok((st, logical))
    }

    /// \[EXTENSION\] `EXPLAIN <cursor specification>`, `sql` the
    /// specification: a cursor of one column, PLAN, whose rows are the
    /// lines `explain` gives, in order. It opens a transaction, as a query
    /// does, but reads no table.
    ///
    /// # Errors
    ///
    /// As `explain`.
    pub(super) fn explain_rows(&self, sql: &str) -> Result<Cursor> {
        self.open_with((commands::EXPLAIN, commands::UNRECOGNIZED), |call| {
            let text = self.planned(call.state(), sql, &Params::new())?;
            self.unnamed(call, None, |st, _| plan_rows(st, &text))
        })
    }

    /// The text `explain` gives of `sql` in state `st`.
    fn planned(&self, st: &Arc<State>, sql: &str, params: &Params) -> Result<String> {
        let spec = Self::parse_cursor(st, sql, params)?;
        let compiled = self.compile(st, &spec, params)?;
        let text = with_count(st, &compiled.physical, shown(&compiled));
        Ok(with_plugin_calls(st, text))
    }
}

/// The column PLAN and the rows of `text`, a line each.
///
/// # Errors
///
/// The rows cannot be held.
fn plan_rows(st: &Arc<State>, text: &str) -> Result<(Vec<OutColumn>, Stream)> {
    let width = text.lines().map(|l| l.chars().count()).max().unwrap_or_default().max(1);
    let column = OutColumn {
        name: Some(PLAN_COLUMN.to_owned()),
        collation: None,
        data_type: DataType::varying(u32::try_from(width).unwrap_or(u32::MAX)),
        not_null: true,
        origin: None,
    };
    let rows = text.lines().map(|l| vec![Value::Char(l.into())]).collect();
    Ok((vec![column], Stream::made(st, rows)?))
}

/// The stages of `c`, each headed by a comment line, the rules that fired
/// with repeats collapsed: "merge_filters x2".
fn shown(c: &Compiled) -> String {
    let mut runs: Vec<(&str, usize)> = Vec::new();
    for rule in &c.trace {
        match runs.last_mut().filter(|(name, _)| name == rule) {
            Some((_, count)) => *count = count.saturating_add(1),
            None => runs.push((rule, 1)),
        }
    }
    let rules: Vec<String> = runs.iter().map(|(r, n)| if *n > 1 { format!("{r} x{n}") } else { (*r).to_owned() }).collect();
    format!(
        "-- bound:\n{}-- rules: {}\n-- optimized:\n{}-- physical:\n{}",
        c.bound,
        if rules.is_empty() { "none".to_owned() } else { rules.join(", ") },
        c.optimized.explain(),
        c.physical.explain()
    )
}

/// \[DESIGN\] `text`, a plan, with a line naming the table whose rows the
/// bare COUNT(*) of `physical` counts by its storage, if it does
/// (RULES.md D351, `exec/batch/count.rs`).
fn with_count(st: &State, physical: &Plan, text: String) -> String {
    match crate::exec::storage_counted(physical, st) {
        Some(table) => format!("{text}-- counted by storage: {table}\n"),
        None => text,
    }
}

/// \[EXTENSION\] `text`, a plan, with a last line naming the objects of
/// extensions it calls, each as its extension's name dotted with its own
/// (RULES.md D172): a function written as called, a metric or model as
/// its name quoted.
fn with_plugin_calls(st: &State, text: String) -> String {
    let upper = text.to_uppercase();
    let mut calls = Vec::new();
    for e in st.extensions.values() {
        for (d, _) in &e.objects {
            let name = d.name.to_uppercase();
            if upper.contains(&format!("{name}(")) || upper.contains(&format!("'{name}'")) {
                calls.push(format!("{}.{}", e.manifest.name, d.name));
            }
        }
    }
    if calls.is_empty() {
        return text;
    }
    format!("{text}-- plug-in calls: {}\n", calls.join(", "))
}
