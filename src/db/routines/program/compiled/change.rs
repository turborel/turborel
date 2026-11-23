//! \[DESIGN\] A change of a compiled program run straight on its table
//! (RULES.md D841): a searched UPDATE or DELETE by a whole unique key,
//! alone or as the statement of a SELECT ... INTO's FINAL, NEW or OLD
//! TABLE, and an INSERT of one row of values.
//!
//! A change runs the general path first, which keeps its plans as a
//! program kept keeps them (RULES.md D406, D792, D793). Once they are
//! kept in a shape it may take, it takes them from there and runs with
//! them, past the statement's parse, binding and wrapping: its object
//! rows by its plan kept as that plan stands, learning or learned
//! (D422), a point read once it is one (D752), and the rows written by
//! the write functions the general path writes them with, so the
//! statement's BEFORE triggers, row locks, rechecks, escrow (D452),
//! constraints checked at its end, defaults, identity and generated
//! columns are the general path's own. A change of a table some trigger
//! of a table it may write is on, of a view, of a typed or partitioned
//! table, or of XML options in scope, runs the general path always.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use super::super::super::super::delta::Collector;
use super::super::super::super::prepared::KeptPlan;
use super::super::super::super::write::{Target, delete_rows, update_rows};
use super::super::super::super::{Completion, Session, State};
use super::evaluated::store_row;
use super::run::Exec;
use super::valued::Stores;
use crate::ast::{DeltaResult, InsertSource, InsertStatement, QueryExpression, SqlStatement, TableReference};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, fail};
use crate::exec::{Framed, Host, eval};
use crate::logical::{Contained, Expr, LogicalPlan};
use crate::storage::Row;

/// \[DESIGN\] How many runs of the general path a change waits for its
/// plans to be kept in a shape it may take, before it runs the general
/// path for good: its plans are kept by its first run, or by its second
/// once its first noted it ran with a value it holds.
const TRIES: u8 = 3;

/// A change of a compiled program, and how it runs once its plans show.
#[derive(Debug)]
pub(in crate::db) struct Change {
    shape: Shape,
    direct: OnceLock<Option<Direct>>,
    tries: AtomicU8,
}

/// What a change is, as written.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// A searched UPDATE, or DELETE if `delete`; of a SELECT ... INTO's
    /// delta table, its result.
    Searched { delete: bool, delta: Option<DeltaResult> },
    /// An INSERT of a query.
    Insert,
}

/// A change as it runs straight on its table.
#[derive(Debug)]
enum Direct {
    Searched(Box<Searched>),
    Insert(Box<Inserted>),
}

/// A searched UPDATE or DELETE by a whole unique key, its kept plans.
#[derive(Debug)]
struct Searched {
    delete: bool,
    /// The base table it changes.
    table: QualifiedName,
    /// Of a SELECT ... INTO's delta table: its result, its select list
    /// over the delta table's row, and where its targets store.
    delta: Option<(DeltaResult, Vec<Expr>, Stores)>,
}

/// An INSERT of one row of values, its kept target and query.
#[derive(Debug)]
struct Inserted {
    target: Arc<Target>,
    query: LogicalPlan,
    clocked: bool,
}

impl Change {
    /// The change `stmt` is, if it is of a shape that may run straight on
    /// its table.
    pub(in crate::db) fn of(stmt: &SqlStatement) -> Option<Change> {
        let shape = shape_of(stmt)?;
        Some(Change { shape, direct: OnceLock::new(), tries: AtomicU8::new(0) })
    }

    /// `contained`, this change, run straight on its table by `exec`, if
    /// its plans are kept in a shape it may take and the frame's values
    /// place its probe; none if it is to run the general path.
    ///
    /// # Errors
    ///
    /// The statement's, as the general path raises them.
    pub(super) fn run(&self, exec: &mut Exec<'_>, contained: &Contained) -> Result<Option<Completion>> {
        match self.direct.get() {
            Some(Some(Direct::Searched(s))) => searched(exec, contained, s),
            Some(Some(Direct::Insert(i))) => inserted(exec, contained, i),
            Some(None) | None => Ok(None),
        }
    }

    /// After a run of the general path of `contained`, whether its plans
    /// are now kept in a shape this change may take; once it waited
    /// `TRIES` runs, it runs the general path for good.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(super) fn learn(&self, exec: &Exec<'_>, contained: &Contained) -> Result<()> {
        if self.direct.get().is_some() {
            return Ok(());
        }
        let found = direct_of(exec, contained, self.shape)?;
        if found.is_some() || self.tries.fetch_add(1, Ordering::Relaxed).saturating_add(1) >= TRIES {
            let _ = self.direct.set(found);
        }
        Ok(())
    }
}

/// The shape of `stmt`, if it may run straight on its table.
fn shape_of(stmt: &SqlStatement) -> Option<Shape> {
    if stmt.xml_options().is_some() {
        return None;
    }
    if let SqlStatement::UpdateSearched { portion: None, .. } = stmt {
        return Some(Shape::Searched { delete: false, delta: None });
    }
    if let SqlStatement::DeleteSearched { portion: None, .. } = stmt {
        return Some(Shape::Searched { delete: true, delta: None });
    }
    if let SqlStatement::Insert(ins) = stmt {
        return inserts_values(ins).then_some(Shape::Insert);
    }
    let SqlStatement::Select { query, .. } = stmt else { return None };
    let [TableReference::Delta(d)] = query.table_expression.from.as_slice() else { return None };
    let te = &query.table_expression;
    let plain = te.where_clause.is_none() && te.group_by.is_none() && te.having.is_none() && d.statement.xml_options().is_none();
    let update = matches!(&*d.statement, SqlStatement::UpdateSearched { portion: None, .. });
    let delete = matches!(&*d.statement, SqlStatement::DeleteSearched { portion: None, .. });
    let fits = match d.result {
        DeltaResult::Final | DeltaResult::New => update,
        DeltaResult::Old => delete,
    };
    (plain && fits).then_some(Shape::Searched { delete, delta: Some(d.result) })
}

/// Whether `ins` inserts the rows of a query that reads no delta table:
/// a VALUES of one row, its kept plan shows.
fn inserts_values(ins: &InsertStatement) -> bool {
    let InsertSource::Query(q) = &ins.source else { return false };
    matches!(q.body(), QueryExpression::Values(_) | QueryExpression::Spec(_)) && q.deltas().is_empty()
}

/// Whether no trigger is on a table a change of `table` may write, its
/// referential actions' too.
fn untriggered(st: &State, table: &QualifiedName) -> bool {
    !st.catalog.any_triggers() || st.written_by(table).iter().all(|t| st.catalog.triggers_on(t).is_empty())
}

/// Whether `table` is a base table a change may write straight: neither
/// typed nor partitioned, and no trigger on what it may write.
fn plain_table(st: &State, table: &QualifiedName) -> bool {
    let Some(desc) = st.catalog.table(table) else { return false };
    desc.typed.is_none() && !st.catalog.partitioning().is_partitioned(table) && untriggered(st, table)
}

/// How `contained`, of shape `shape`, runs straight, from the plans the
/// general path kept of it; none if they are not kept so.
fn direct_of(exec: &Exec<'_>, contained: &Contained, shape: Shape) -> Result<Option<Direct>> {
    let (session, st) = (exec.session, exec.cx.st);
    let Some((plans, delta_query)) = exec.kept.plans_of(contained) else { return Ok(None) };
    let keeping = session.keeping(st, plans);
    let Shape::Searched { delete, delta } = shape else {
        let Some((KeptPlan::Insert(target, Some(query)), clocked)) = keeping.plan_clocked()? else { return Ok(None) };
        if !one_row(&query) || !plain_table(st, target.name()) || named_view(exec, contained) {
            return Ok(None);
        }
        return Ok(Some(Direct::Insert(Box::new(Inserted { target, query, clocked }))));
    };
    let Some(bound) = keeping.kept_write()? else { return Ok(None) };
    if bound.named != bound.table || !plain_table(st, &bound.table) {
        return Ok(None);
    }
    let table = bound.table.clone();
    let Some(result) = delta else { return Ok(Some(Direct::Searched(Box::new(Searched { delete, table, delta: None })))) };
    let Some(KeptPlan::Query(_, plan)) = keeping.querying(Some(delta_query)).query().map(|q| q.plan()).transpose()?.flatten() else {
        return Ok(None);
    };
    let (Some(outputs), SqlStatement::Select { targets, .. }) = (outputs_of(&plan), contained.statement()) else { return Ok(None) };
    let stores = Stores::of_specs(targets, &contained.scope);
    Ok(Some(Direct::Searched(Box::new(Searched { delete, table, delta: Some((result, outputs, stores)) }))))
}

/// Whether the INSERT `contained` names a view, whose INSTEAD OF trigger
/// may take it.
fn named_view(exec: &Exec<'_>, contained: &Contained) -> bool {
    let SqlStatement::Insert(ins) = contained.statement() else { return true };
    let name = crate::binder::qualify(&ins.table, &exec.session.default_schema());
    exec.cx.st.catalog.table(&name).is_none()
}

/// Whether `plan`, an INSERT's query, is a VALUES of one row, projected
/// or not, whose values read nothing.
fn one_row(plan: &LogicalPlan) -> bool {
    let (values, projected) = if let LogicalPlan::Project { exprs, input } = plan { (&**input, Some(exprs)) } else { (plan, None) };
    let LogicalPlan::Values { rows, .. } = values else { return false };
    rows.len() == 1 && rows.iter().flatten().chain(projected.into_iter().flatten()).all(Expr::reads_nothing)
}

/// The select list of a query over its delta table alone, as expressions
/// over the delta table's row, each reading nothing but it: none if the
/// query is of another shape.
fn outputs_of(plan: &LogicalPlan) -> Option<Vec<Expr>> {
    let LogicalPlan::Project { exprs, input } = plan else { return None };
    let outputs = if let LogicalPlan::Transition { .. } = &**input {
        exprs.clone()
    } else {
        let LogicalPlan::Project { exprs: inner, input } = &**input else { return None };
        let LogicalPlan::Transition { .. } = &**input else { return None };
        let columns = inner.iter().map(|e| if let Expr::Column(c) = e { Some(*c) } else { None }).collect::<Option<Vec<usize>>>()?;
        exprs.iter().map(|e| remapped(e.clone(), &columns)).collect::<Option<Vec<Expr>>>()?
    };
    outputs.iter().all(Expr::reads_nothing).then_some(outputs)
}

/// `e`, over a row whose column `i` is column `columns[i]` of the delta
/// table's, as an expression over the delta table's row; none if it reads
/// a column the row has not.
fn remapped(e: Expr, columns: &[usize]) -> Option<Expr> {
    let missing = std::cell::Cell::new(false);
    let mapped = remap(e, columns, &missing);
    (!missing.get()).then_some(mapped)
}

/// `remapped`'s work, a column it cannot place noted in `missing`.
fn remap(e: Expr, columns: &[usize], missing: &std::cell::Cell<bool>) -> Expr {
    if let Expr::Column(c) = e {
        return columns.get(c).map_or_else(
            || {
                missing.set(true);
                Expr::Column(c)
            },
            |&k| Expr::Column(k),
        );
    }
    e.map_children(&mut |child| remap(child, columns, missing))
}

/// `contained`, the searched change `s`, run straight on its table.
fn searched(exec: &mut Exec<'_>, contained: &Contained, s: &Searched) -> Result<Option<Completion>> {
    let (session, cx) = (exec.session, exec.cx);
    let Some((plans, _)) = exec.kept.plans_of(contained) else { return Ok(None) };
    exec.frame.set_routine(Arc::clone(&contained.scope));
    exec.host.fresh(cx.tx)?;
    session.may_run(cx.st, change_of(contained.statement()))?;
    let keeping = session.keeping(cx.st, plans);
    let Some(w) = keeping.written_kept(exec.frame.dynamic_slots().unwrap_or_default())? else { return Ok(None) };
    if w.bound.table != s.table {
        return Ok(None);
    }
    let collector = s.delta.as_ref().map(|(result, _, _)| Collector::of_base(cx.st, &s.table, *result));
    let outer = collector.clone().map(|c| session.delta_replaced(Some(c)));
    let ran = (|| {
        let _compiling = session.compiling(cx.st, cx.tx.id);
        session.settle_cursors(cx.st, &s.table);
        let mut effects = session.effects(cx.st);
        let done = if s.delete { delete_rows(cx, w, &mut effects)? } else { update_rows(cx, w, &mut effects)? };
        if done.is_some() {
            cx.st.end_statement(cx.tx, effects)?;
        }
        Ok(session.affected(done.unwrap_or(0)))
    })();
    if let Some(outer) = outer {
        session.delta_replaced(outer);
    }
    let done = ran?;
    let (Some((_, outputs, stores)), Some(collector)) = (&s.delta, collector) else { return Ok(Some(done)) };
    selected(exec, contained, (outputs, stores), &collector).map(Some)
}

/// The statement whose rows `stmt` changes: of a SELECT ... INTO of a
/// delta table, its statement; else itself.
fn change_of(stmt: &SqlStatement) -> &SqlStatement {
    if let SqlStatement::Select { query, .. } = stmt
        && let [TableReference::Delta(d)] = query.table_expression.from.as_slice()
    {
        return &d.statement;
    }
    stmt
}

/// [X3.135-1986 §8.10] SELECT ... INTO of the delta table's rows
/// `collector` gathered: its select list `outputs` of the one row
/// assigned to its targets; of none, no data; of more than one,
/// SQLSTATE 21000.
fn selected(
    exec: &mut Exec<'_>,
    contained: &Contained,
    (outputs, stores): (&[Expr], &Stores),
    collector: &Collector,
) -> Result<Completion> {
    let mut held = collector.taken(exec.cx.st);
    let mut rows: Vec<Row> = Vec::with_capacity(2);
    // The select list reads the frame's variables and parameters as slots.
    let framed = Framed { inner: &exec.hosted, frame: exec.frame.dynamic_values(), outer: &[] };
    held.visit(&mut |row| {
        if rows.len() < 2 {
            rows.push(outputs.iter().map(|e| eval(e, row, &framed)).collect::<Result<Row>>()?);
        }
        Ok(())
    })?;
    let SqlStatement::Select { targets, .. } = contained.statement() else { return Ok(Completion::Success) };
    match rows.as_mut_slice() {
        [] => Ok(Completion::NoData),
        [row] => {
            exec.host.present(row);
            store_row(exec.frame, std::mem::take(row), (targets, stores))
        }
        [_, _, ..] => fail(Code::Cardinality, "[X3.135-1986 §8.10 GR2] the result has more than one row"),
    }
}

/// `contained`, the INSERT `i`, run with its kept target and query.
fn inserted(exec: &mut Exec<'_>, contained: &Contained, i: &Inserted) -> Result<Option<Completion>> {
    let (session, cx) = (exec.session, exec.cx);
    let SqlStatement::Insert(stmt) = contained.statement() else { return Ok(None) };
    let Some((plans, _)) = exec.kept.plans_of(contained) else { return Ok(None) };
    if exec.frame.given().is_some() || exec.frame.load().is_some() {
        return Ok(None);
    }
    exec.frame.set_routine(Arc::clone(&contained.scope));
    exec.host.fresh(cx.tx)?;
    session.may_run(cx.st, contained.statement())?;
    let authorization = session.authorization();
    let target = Target::clone(&i.target).refreshed(authorization.of(cx.st.catalog.roles()), session.now.get())?;
    let keeping = session.keeping(cx.st, plans);
    let slots = exec.frame.dynamic_slots().map(|s| keeping.slots_of(s, i.clocked));
    let _compiling = session.compiling(cx.st, cx.tx.id);
    let kept = (!i.clocked).then_some((&keeping, &*exec.frame));
    let n = session.insert_rows(cx, (stmt, &target), (Some(i.query.clone()), slots, kept), exec.frame)?;
    Ok(Some(session.affected(n)))
}

/// Whether `session` runs a procedure's body compiled
/// (EXECUTOR.COMPILED_PROCEDURES, RULES.md D841).
pub(in crate::db) fn compiled_procedures(st: &State, session: &Session) -> bool {
    st.settings.is_on_for(crate::settings::executor::COMPILED_PROCEDURES, &session.own.borrow())
}

#[cfg(test)]
mod tests;
