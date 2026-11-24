//! \[EXTENSION\] EXPLAIN of a data change statement without ANALYZE,
//! INSERT, UPDATE, DELETE or MERGE (RULES.md D980): what it would do,
//! found without running it. It is bound as preparing it binds it, so it
//! needs the privileges that needs and no other (D981). Its rows are
//! planned as it would plan them, and nothing is read, locked or written,
//! so no trigger fires and no row or table is locked (D982).
//!
//! Its text is that of EXPLAIN ANALYZE of the statement without what ran
//! (`explain/changed.rs`): a line naming the statement and its table, the
//! plan of the rows it reads below it, if it reads them by one. Lines
//! after it say what the change would do beside (D983): the indexes it
//! maintains, the triggers that would fire, the referential actions that
//! would cascade (`unrun/effects.rs`), and whether a point path runs it
//! where it is prepared or a procedure's (D984).

mod effects;

use std::fmt::Write;
use std::sync::Arc;

use self::effects::Changes;
use super::super::merge::joined_rows;
use super::super::routines::{compiled_procedures, evaluates, runs_straight};
use super::super::triggers::{Event, instead_for};
use super::super::dynamic::Name;
use super::super::{Session, State, api, plan, plan_written, rule};
use super::changed::label_of;
use super::with_plugin_calls;
use crate::ast::{InsertSource, InsertStatement, MergeStatement, MergeWhen, SetClause, SqlStatement, TableName};
use crate::binder::{BoundWhen, SearchedTarget, WriteKind, bind_searched_write, bind_write, qualify};
use crate::catalog::{QualifiedName, TriggerDesc};
use crate::error::{Code, Result, SqlError};
use crate::exec::{PointRead, point_operations};
use crate::logical::LogicalPlan;
use crate::params::Params;
use crate::physical::{PhysicalPlanner, Plan};
use crate::settings::FUNCTION_CALLS;

/// What EXPLAIN says of a change no path but the general one runs.
const NO_POINT_PATH: &str = "none";

/// A data change as EXPLAIN shows it, found without running it.
struct Unrun {
    /// The plan of the rows it reads, if it reads them by one; or the
    /// line of what a positioned change reads, its cursor's row.
    rows: Rows,
    /// What it changes, if it changes a table itself.
    changes: Option<Changes>,
    /// The INSTEAD OF triggers that would take it in place of the change.
    instead: Vec<TriggerDesc>,
    /// How a point path would run it, each a phrase.
    point: Vec<String>,
}

/// What a data change reads its rows by.
enum Rows {
    /// No plan: its values are evaluated, or a trigger takes it.
    None,
    /// The plan of its rows, described as EXPLAIN shows it.
    Plan(Box<Plan>),
    /// The row of a cursor.
    Cursor(String),
}

impl Unrun {
    /// Of a change an INSTEAD OF trigger of `instead` takes, if one does.
    fn instead(instead: Vec<TriggerDesc>) -> Option<Unrun> {
        (!instead.is_empty()).then_some(Unrun { rows: Rows::None, changes: None, instead, point: Vec::new() })
    }

    /// Its text, its first line `label`, as `with_plugin_calls` ends it.
    fn text(&self, st: &State, label: &str) -> String {
        let mut text = format!("-- physical:\n{label}\n");
        // Writing to a String cannot fail.
        match &self.rows {
            Rows::None => {}
            Rows::Plan(plan) => {
                for line in plan.explain().lines() {
                    let _ = writeln!(text, "  {line}");
                }
            }
            Rows::Cursor(cursor) => {
                let _ = writeln!(text, "  CurrentRow of cursor {cursor}");
            }
        }
        if let Some(changes) = &self.changes {
            effects::shown(st, changes, &mut text);
        }
        for t in &self.instead {
            let _ = writeln!(text, "{}", effects::trigger_line(t));
        }
        let point = if self.point.is_empty() { NO_POINT_PATH.to_owned() } else { self.point.join(", ") };
        let _ = writeln!(text, "-- point path: {point}");
        with_plugin_calls(st, text)
    }
}

impl Session {
    /// \[EXTENSION\] EXPLAIN of `sql`, a data change statement, its
    /// parameters `params`, against the definitions `st`: what it would
    /// do, as `unrun.rs` finds it, with nothing run.
    ///
    /// # Errors
    ///
    /// Any error of parsing or binding the statement, as preparing it
    /// raises them; SQLCODE -900 of MERGE INTO PROPERTY.
    pub(in crate::db) fn unrun_change(&self, st: &Arc<State>, sql: &str, params: &Params) -> Result<String> {
        let lexed = crate::lexer::lexed(sql)?;
        let stmt = params.parser(lexed, st.settings.is_on(FUNCTION_CALLS), &st.catalog_name()).sql_statement()?;
        let label = label_of(&stmt, &self.default_schema());
        let unrun = if let SqlStatement::Insert(ins) = &stmt {
            self.unrun_insert(st, &stmt, ins, params)?
        } else if let SqlStatement::UpdateSearched { table, only, portion, set, where_clause, .. } = &stmt {
            let target = (table, *only, portion.as_deref());
            self.unrun_searched(st, &stmt, (WriteKind::UpdateSearched, target, set, where_clause.as_ref()), params)?
        } else if let SqlStatement::DeleteSearched { table, only, portion, where_clause, .. } = &stmt {
            let target = (table, *only, portion.as_deref());
            self.unrun_searched(st, &stmt, (WriteKind::DeleteSearched, target, &[], where_clause.as_ref()), params)?
        } else if let SqlStatement::UpdatePositioned { table, only, set, cursor, .. } = &stmt {
            self.unrun_positioned(st, (WriteKind::UpdatePositioned, (table, *only), set, &cursor.0), params)?
        } else if let SqlStatement::DeletePositioned { table, only, cursor } = &stmt {
            self.unrun_positioned(st, (WriteKind::DeletePositioned, (table, *only), &[], &cursor.0), params)?
        } else if let SqlStatement::Merge(merge) = &stmt {
            self.unrun_merge(st, merge, params)?
        } else {
            return Err(api("[DESIGN] EXPLAIN of a data change is of INSERT, UPDATE, DELETE or MERGE"));
        };
        Ok(unrun.text(st, &label))
    }

    /// The INSTEAD OF triggers that take a change of `named`, of each of
    /// `events`, as running it would find them.
    fn taken_instead(&self, st: &State, named: &TableName, events: &[Event]) -> Vec<TriggerDesc> {
        let name = qualify(named, &self.default_schema());
        let mut found: Vec<TriggerDesc> = Vec::new();
        for instead in events.iter().filter_map(|&e| instead_for(st, &name, e)) {
            if !found.iter().any(|t| t.name == instead.trigger.name) {
                found.push(instead.trigger);
            }
        }
        found
    }

    /// INSERT `ins`, the statement `stmt`, as EXPLAIN shows it.
    fn unrun_insert(&self, st: &State, stmt: &SqlStatement, ins: &InsertStatement, params: &Params) -> Result<Unrun> {
        if let Some(unrun) = Unrun::instead(self.taken_instead(st, &ins.table, &[Event::Insert])) {
            return Ok(unrun);
        }
        let target = self.insert_target(st, ins, (params.noted(), params.horizon()))?;
        // [ISO/IEC 9075:1992 §13.8 SR2] DEFAULT VALUES, a row made of its
        // defaults; a row of values that reads nothing, evaluated (RULES.md D419).
        let logical = match &ins.source {
            InsertSource::Query(query) => Some(self.insert_query(st, &target, query, params)?).filter(|l| !evaluates(l)),
            InsertSource::DefaultValues => None,
        };
        let mut point = Vec::new();
        if logical.is_none() {
            point.push("its values evaluated, with no plan".to_owned());
        }
        let named = qualify(&ins.table, &self.default_schema());
        if self.straight_in_procedure(st, stmt, (&named, target.name())) && target.straight(st).is_some() {
            point.push("its rows written straight in a procedure".to_owned());
        }
        let rows = logical.map_or(Rows::None, |l| Rows::Plan(Box::new(described(st, &plan(st, l).0))));
        let changes = Changes { table: target.name().clone(), events: vec![(Event::Insert, target.object().to_vec())] };
        Ok(Unrun { rows, changes: Some(changes), instead: Vec::new(), point })
    }

    /// A searched UPDATE or DELETE, `kind`, the statement `stmt`, of
    /// `target` giving `set` where `cond` is true, as EXPLAIN shows it.
    fn unrun_searched(
        &self,
        st: &State,
        stmt: &SqlStatement,
        (kind, target, set, cond): (WriteKind, SearchedTarget<'_>, &[SetClause], Option<&crate::ast::SearchCondition>),
        params: &Params,
    ) -> Result<Unrun> {
        let delete = matches!(kind, WriteKind::DeleteSearched);
        let event = if delete { Event::Delete } else { Event::Update };
        if let Some(unrun) = Unrun::instead(self.taken_instead(st, target.0, &[event])) {
            return Ok(unrun);
        }
        let bound = self.bound(st, params, |b| bind_searched_write(b, kind, target, set, cond))?;
        let (optimized, _, physical) = plan_written(st, bound.rows.clone());
        // [DESIGN] RULES.md D752: one read of a whole unique key, as a
        // prepared statement's plan kept or a procedure's runs it.
        let read = PointRead::of(&physical, st).filter(|_| point_operations(st));
        let mut point: Vec<String> = read.iter().map(|p| format!("one read of {} when prepared or in a procedure", p.index())).collect();
        let straight = st.straight(&bound, delete).is_some() && self.straight_in_procedure(st, stmt, (&bound.named, &bound.table));
        if read.is_some() && straight {
            point.push("its row written straight in a procedure".to_owned());
        }
        let columns = if delete { Vec::new() } else { super::super::assignment::set_columns_of(&bound.set, &bound.generated) };
        let changes = Changes { table: bound.table.clone(), events: vec![(event, columns)] };
        Ok(Unrun { rows: Rows::Plan(Box::new(described(st, &optimized))), changes: Some(changes), instead: Vec::new(), point })
    }

    /// A positioned UPDATE or DELETE, `kind`, of `table` giving `set`
    /// through the cursor `cursor`, as EXPLAIN shows it: no plan, but the
    /// cursor's row.
    fn unrun_positioned(
        &self,
        st: &State,
        (kind, table, set, cursor): (WriteKind, (&TableName, bool), &[SetClause], &str),
        params: &Params,
    ) -> Result<Unrun> {
        let delete = matches!(kind, WriteKind::DeletePositioned);
        // [X3.135-1986 §8.4 SR2, §8.11 SR2] its cursor declared, open or not.
        let key = params.local_cursor(cursor).map_or_else(|| self.positioned_key(cursor), |key| Name::plain(&key));
        if !self.inner.borrow().cursors.contains_key(&key) {
            return Err(rule(format!("[X3.135-1986 {} SR2] no cursor {key} is declared", if delete { "§8.4" } else { "§8.11" })));
        }
        let bound = self.bound(st, params, |b| bind_write(b, kind, table, set, None))?;
        let event = if delete { Event::Delete } else { Event::Update };
        if let Some(unrun) = Unrun::instead(self.taken_instead(st, table.0, &[event])) {
            return Ok(unrun);
        }
        let columns = if delete { Vec::new() } else { super::super::assignment::set_columns_of(&bound.set, &bound.generated) };
        let changes = Changes { table: bound.table.clone(), events: vec![(event, columns)] };
        Ok(Unrun { rows: Rows::Cursor(cursor.to_owned()), changes: Some(changes), instead: Vec::new(), point: Vec::new() })
    }

    /// MERGE `merge` as EXPLAIN shows it: the plan of its joined rows, and
    /// what each of its clauses changes.
    fn unrun_merge(&self, st: &State, merge: &MergeStatement, params: &Params) -> Result<Unrun> {
        if merge.property.is_some() {
            let msg = "[EXTENSION] EXPLAIN shows no MERGE INTO PROPERTY, whose statements are made as it runs";
            return Err(SqlError::new(Code::Unsupported, msg));
        }
        let written: Vec<Event> = merge.clauses.iter().map(clause_event).collect();
        let instead = self.taken_instead(st, &merge.target, &written);
        let (bound, _) = self.merge_bound(st, merge, params, None)?;
        let rows = Rows::Plan(Box::new(described(st, &plan(st, joined_rows(&bound)?).0)));
        let generated = |n: usize| bound.generated.get(n).map_or(&[][..], Vec::as_slice);
        let by_clause = bound.clauses.iter().enumerate().map(|(n, c)| match c {
            BoundWhen::Matched { action: crate::binder::BoundAction::Update(set), .. } => {
                (Event::Update, super::super::assignment::set_columns_of(set, generated(n)))
            }
            BoundWhen::Matched { action: crate::binder::BoundAction::Delete, .. } => (Event::Delete, Vec::new()),
            BoundWhen::NotMatched { object, .. } => (Event::Insert, object.clone()),
        });
        // A clause an INSTEAD OF trigger takes changes no table itself.
        let events = by_clause.filter(|(e, _)| instead.is_empty() || instead_for(st, &bound.named, *e).is_none()).collect();
        let changes = Changes { table: bound.table.clone(), events };
        Ok(Unrun { rows, changes: Some(changes), instead, point: Vec::new() })
    }

    /// Whether a procedure's body would run `stmt`, changing `table`
    /// named as `named`, straight on its table (RULES.md D841).
    fn straight_in_procedure(&self, st: &State, stmt: &SqlStatement, (named, table): (&QualifiedName, &QualifiedName)) -> bool {
        compiled_procedures(st, self) && runs_straight(st, stmt, (named, table))
    }
}

/// The trigger event of the WHEN clause `clause`.
fn clause_event(clause: &MergeWhen) -> Event {
    match clause {
        MergeWhen::Matched { action: crate::ast::MatchedAction::Update(_), .. } => Event::Update,
        MergeWhen::Matched { action: crate::ast::MatchedAction::Delete, .. } => Event::Delete,
        MergeWhen::NotMatched { .. } => Event::Insert,
    }
}

/// `optimized`, a plan optimized, planned again by the planner that
/// describes each operator as EXPLAIN shows it, choosing as the planner
/// that runs it chooses, as EXPLAIN ANALYZE plans it (`changed.rs`).
fn described(st: &State, optimized: &LogicalPlan) -> Plan {
    PhysicalPlanner::describing(st).plan(optimized)
}
