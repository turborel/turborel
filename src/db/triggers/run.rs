//! [ISO/IEC 9075-2:2023 §15.21 GR5, GR6] A triggered action invoked: its
//! WHEN condition evaluated, then each statement of its triggered SQL
//! statement executed, as a statement of its own [§9.17], in the context
//! the trigger pushes on the session.
//!
//! As the action runs, the session's diagnostics area is one of its own,
//! discarded after, so the statement that caused it reads its own [GR6a,
//! GR6k]; its authorization identifier is the owner of the trigger's
//! schema [GR6b, GR6l], its default schema the one the trigger's names
//! were defined in, and no session collation applies, as of a view. Its
//! statement timestamp is the statement's that caused it [§9.17 GR5]. No
//! savepoint is established [GR6c, GR6j]: one that fails fails that
//! statement, undone whole [§4.46.2], so its own changes are undone too.
//!
//! Each statement keeps its plans, bound once with the action's
//! transition variables as slots and the handles of its transition
//! tables, filled at each invocation (RULES.md D45), as a prepared
//! statement's are: kept per session, trigger and depth of nesting, so an
//! action nested in its own invocation reads its own transition tables.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use super::super::collations::Collating;
use super::super::prepared::Plans;
use super::super::{Session, State, StatementContext, area};
use super::MAX_TRIGGER_DEPTH;
use crate::ast::{SchemaStatement, SearchCondition, SqlStatement, TriggerDefinition};
use crate::binder::bind_when;
use crate::catalog::{ActionTime, Catalog, ColumnDesc, QualifiedName, TriggerDesc, TriggerLevel, owner_of};
use crate::error::{Code, Result, SqlError, SqlState, Subject};
use crate::exec::{HeldRows, Runner, TransitionRows, eval};
use crate::logical::{Expr, filled_expr};
use crate::params::{Params, Transitions};
use crate::parser::Parser;
use crate::storage::Row;
use crate::sync::lock_builtin;
use crate::value::Value;

/// The triggered actions a session is executing, innermost last, and the
/// actions it keeps, each bound for a trigger at a depth of nesting.
#[derive(Default)]
pub(in crate::db) struct Triggering {
    executing: RefCell<Vec<QualifiedName>>,
    kept: RefCell<HashMap<(QualifiedName, u64, usize), Arc<Action>>>,
}

/// A triggered action, read from its trigger's text, as one depth of
/// nesting runs it: what it was read with, its WHEN condition and its
/// statements, their kept plans, and its transition variables and tables.
struct Action {
    catalog: Weak<dyn Catalog>,
    when: Option<SearchCondition>,
    /// The WHEN condition bound with its slots, once bound so.
    bound_when: Mutex<Option<Expr>>,
    statements: Vec<SqlStatement>,
    plans: Vec<Plans>,
    transitions: Arc<Transitions>,
}

/// What one invocation of a triggered action is given [§15.21 GR5a]: the
/// values of its old and new transition variables, the subject table's
/// columns each, and the rows of its old and new transition tables.
pub(in crate::db) struct Invocation {
    pub(in crate::db) values: Vec<Value>,
    pub(in crate::db) old_table: Option<HeldRows<Row>>,
    pub(in crate::db) new_table: Option<HeldRows<Row>>,
}

/// What a session was, before a trigger pushed its context on it: put
/// back as it is dropped.
struct Pushed<'s> {
    session: &'s Session,
    auth: String,
    schema: String,
    collating: Collating,
    area: area::Area,
}

impl Drop for Pushed<'_> {
    fn drop(&mut self) {
        let s = self.session;
        *s.auth.borrow_mut() = std::mem::take(&mut self.auth);
        *s.schema.borrow_mut() = std::mem::take(&mut self.schema);
        s.prepared_collation.replace(std::mem::replace(&mut self.collating, Collating::AsSet));
        s.inner.borrow_mut().area = std::mem::take(&mut self.area);
        s.triggering.executing.borrow_mut().pop();
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §15.21 GR5, GR6] Invokes the triggered action
    /// of trigger `t` in `cx`, with `given`: the new row as its statements
    /// left it, of a BEFORE row trigger, if they ran.
    ///
    /// # Errors
    ///
    /// SQLSTATE 09000, about the trigger, with the SQLCODE of the error of
    /// its WHEN condition or of a statement that failed [GR5b i 1, GR6m];
    /// so too past `MAX_TRIGGER_DEPTH` nested triggered actions.
    pub(in crate::db) fn invoke(&self, cx: StatementContext<'_>, t: &TriggerDesc, given: Invocation) -> Result<Option<Vec<Value>>> {
        self.invoked(cx, t, given).map_err(|e| triggered_exception(t, &e))
    }

    /// `invoke`'s work, its error not yet the trigger's.
    fn invoked(&self, cx: StatementContext<'_>, t: &TriggerDesc, given: Invocation) -> Result<Option<Vec<Value>>> {
        let depth = self.triggering.executing.borrow().len();
        if depth >= MAX_TRIGGER_DEPTH {
            let msg = format!("[DESIGN] triggered actions nest at most {MAX_TRIGGER_DEPTH} deep: trigger {} would nest deeper", t.name);
            return Err(SqlError::new(Code::Unsupported, msg));
        }
        let action = self.action(cx.st, t, depth)?;
        let Invocation { values, old_table, new_table } = given;
        give(action.transitions.old_table.as_ref().map(|(_, rows)| rows), old_table)?;
        give(action.transitions.new_table.as_ref().map(|(_, rows)| rows), new_table)?;
        let mut params = Params::transitional(values, Arc::clone(&action.transitions));
        let _pushed = self.push(cx.st, t);
        if !self.when_holds(cx, &action, &params)? {
            return Ok(None);
        }
        for (stmt, plans) in action.statements.iter().zip(&action.plans) {
            self.may_run(cx.st, stmt)?;
            let keeping = self.keeping(cx.st, plans);
            self.run_statement(cx, stmt, &mut params, Some(&keeping))?;
        }
        Ok(params.new_row())
    }

    /// [ISO/IEC 9075-2:2023 §15.21 GR6a to GR6d] Pushes trigger `t`'s
    /// context on the session, as the module says: popped as what is
    /// returned is dropped.
    fn push(&self, st: &State, t: &TriggerDesc) -> Pushed<'_> {
        let owner = owner_of(&*st.catalog, &t.name.auth).to_owned();
        self.triggering.executing.borrow_mut().push(t.name.clone());
        Pushed {
            session: self,
            auth: self.auth.replace(owner),
            schema: self.schema.replace(t.schema.clone()),
            collating: self.prepared_collation.replace(Collating::Fixed(None)),
            area: std::mem::take(&mut self.inner.borrow_mut().area),
        }
    }

    /// [ISO/IEC 9075-2:2023 §15.21 GR5b] Whether `action` has no WHEN
    /// condition, or its condition is true with `params`.
    fn when_holds(&self, cx: StatementContext<'_>, action: &Action, params: &Params) -> Result<bool> {
        let Some(when) = &action.when else { return Ok(true) };
        let kept = lock_builtin(&action.bound_when).clone();
        let e = if let Some(e) = kept {
            filled_expr(e, params.dynamic_values())?
        } else {
            let slotted = params.keeping();
            let e = self.bound(cx.st, &slotted, |b| bind_when(b, when))?;
            if slotted.held() {
                self.bound(cx.st, params, |b| bind_when(b, when))?
            } else {
                *lock_builtin(&action.bound_when) = Some(e.clone());
                filled_expr(e, params.dynamic_values())?
            }
        };
        Ok(matches!(eval(&e, &Row::new(), &Runner::new(cx.st, cx.tx.id))?, Value::Bool(true)))
    }

    /// The action of trigger `t` as depth `depth` runs it: kept, if what
    /// it was read with holds, else read again.
    fn action(&self, st: &State, t: &TriggerDesc, depth: usize) -> Result<Arc<Action>> {
        let key = (t.name.clone(), t.created, depth);
        let kept = self.triggering.kept.borrow().get(&key).cloned();
        if let Some(action) = kept.filter(|a| std::ptr::addr_eq(Weak::as_ptr(&a.catalog), Arc::as_ptr(&st.catalog))) {
            return Ok(action);
        }
        let action = Arc::new(read_action(st, t)?);
        self.triggering.kept.borrow_mut().insert(key, Arc::clone(&action));
        Ok(action)
    }
}

/// [ISO/IEC 9075-2:2023 §15.21 GR5a] Gives `rows` to the transition table
/// `table`, if the action reads one.
fn give(table: Option<&TransitionRows>, rows: Option<HeldRows<Row>>) -> Result<()> {
    match (table, rows) {
        (Some(table), Some(rows)) => table.give(rows),
        (Some(_) | None, None) | (None, Some(_)) => Ok(()),
    }
}

/// The triggered action of trigger `t`, read from its text in `st`.
///
/// # Errors
///
/// Its text does not parse, or its subject table is gone.
fn read_action(st: &State, t: &TriggerDesc) -> Result<Action> {
    let definition = parsed(&t.text)?;
    let plans = definition.action.iter().map(|_| Plans::default()).collect();
    Ok(Action {
        catalog: Arc::downgrade(&st.catalog),
        when: definition.when,
        bound_when: Mutex::new(None),
        statements: definition.action,
        plans,
        transitions: Arc::new(transitions_of(st, t)?),
    })
}

/// The columns of the subject table of trigger `t` its action sees: as
/// many of the table's in `st` as it had when the trigger was defined
/// [ISO/IEC 9075-2:2023 §11.21 GR2, Note 561], a view's all.
///
/// # Errors
///
/// SQLCODE -910 if its subject table is gone.
pub(super) fn subject_columns(st: &State, t: &TriggerDesc) -> Result<Vec<ColumnDesc>> {
    let columns = st.catalog.table(&t.table).map(|d| d.columns.clone()).or_else(|| st.catalog.view(&t.table).map(|v| v.columns.clone()));
    let mut columns = columns.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] trigger {} has no table {}", t.name, t.table)))?;
    if let Some(width) = t.horizon.width(&t.table) {
        columns.truncate(width);
    }
    Ok(columns)
}

/// The transition variables and tables of trigger `t`, of the columns of
/// its subject table it sees in `st`, each table's rows held in `st`'s
/// space.
///
/// # Errors
///
/// SQLCODE -910 if its subject table is gone.
pub(super) fn transitions_of(st: &State, t: &TriggerDesc) -> Result<Transitions> {
    let columns = subject_columns(st, t)?;
    let table = |name: &Option<String>| name.as_ref().map(|n| (n.clone(), TransitionRows::new(st.list_space())));
    let transitions = Transitions {
        subject: t.table.clone(),
        columns,
        old_row: t.names.old_row.clone(),
        new_row: t.names.new_row.clone(),
        old_table: table(&t.names.old_table),
        new_table: table(&t.names.new_table),
        assigns: t.time == ActionTime::Before && t.level == TriggerLevel::Row,
        horizon: t.horizon.clone(),
        beside: None,
    };
    Ok(transitions)
}

/// The `<trigger definition>` `text`, parsed.
///
/// # Errors
///
/// It does not parse, or is no trigger definition.
pub(super) fn parsed(text: &str) -> Result<TriggerDefinition> {
    let SqlStatement::Schema { statement: SchemaStatement::Trigger(t), .. } = Parser::new(text)?.of_definition().sql_statement()? else {
        return Err(SqlError::new(Code::Api, "[DESIGN] a trigger's text is no <trigger definition>"));
    };
    Ok(t)
}

/// [ISO/IEC 9075-2:2023 §15.21 GR6m, §23.1 GR4g] The triggered action
/// exception of trigger `t`, raised for `cause`: SQLSTATE 09000, about the
/// trigger and its table, its SQLCODE and its text the cause's.
fn triggered_exception(t: &TriggerDesc, cause: &SqlError) -> SqlError {
    let msg = format!(
        "[ISO/IEC 9075-2:2023 §15.21 GR6m] the triggered action of trigger {} failed: SQLSTATE {}: {}",
        t.name,
        cause.sqlstate().as_str(),
        cause.described()
    );
    SqlError::new(cause.code, msg).with_state(SqlState::TRIGGERED_ACTION_EXCEPTION).about(Subject::trigger(t.name.parts(), t.table.parts()))
}
