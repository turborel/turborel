//! Execution of physical plans: the General Rules that produce rows. The FROM
//! product [X3.135-1986 §5.20 GR2b], WHERE and HAVING [X3.135-1986 §5.21 GR2,
//! §5.23], grouping and set functions [X3.135-1986 §5.22, §5.8], the select
//! list and DISTINCT [X3.135-1986 §5.25 GR2, GR3], UNION and ORDER BY
//! [X3.135-1986 §8.3 GR2b, GR3b], EXCEPT, INTERSECT and table value
//! constructors [ISO/IEC 9075:1992 §7.10, §7.2], with three-valued logic
//! [X3.135-1986 §5.18].
//!
//! \[DESIGN\] Each plan node becomes an iterator over rows (the Volcano /
//! iterator model). Rows are pulled one at a time from the top. Operators
//! that must hold rows (sorts, grouping, DISTINCT, hash joins, EXCEPT and
//! INTERSECT) hold up to WORK_MEMORY of them in memory and spill the rest
//! (`exec/work.rs`, RULES.md D38).

mod batch;
mod distinct;
mod eval;
mod group;
mod join;
mod partition;
mod set;
mod sort;
mod subquery;
mod work;

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::{Key, Probe};
use crate::logical::{Cond, Expr, SortKey, Truth};
use crate::physical::Plan;
use crate::storage::{Condition, Row, RowId, TableStorage, project};
use crate::system::SystemRead;
use crate::txn::{TxId, checked};
use crate::value::{CompOp, Value};

pub use eval::{eval, eval_cond, quantified};
pub use join::{DEFAULT_PARTITION_DEPTH, DEFAULT_PARTITIONS};
pub use sort::compare_rows;
pub use subquery::{DEFAULT_DECORRELATE_AFTER, NoSubqueries, Runner, Subqueries, SubqueryRows};

/// The rows a plan produces, each read or computed when it is pulled. The
/// iterator owns what it reads (the state, through an `Arc`, and the
/// tables' scans), so it can be kept between calls: a cursor pulls rows as
/// it is fetched from.
pub type RowIter = Box<dyn Iterator<Item = Result<Row>> + Send>;

/// \[DESIGN\] A broken invariant between the planner and the executor: a plan
/// that names something the state does not have. SQLCODE -910, never a panic
/// (RULES.md R-ERR-1).
fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what}"))
}

/// The rows of a plan that failed as it opened: just its error.
fn failed(e: SqlError) -> RowIter {
    Box::new(std::iter::once(Err(e)))
}

/// Rows are read as transaction `tx` sees them. A failure is the iterator's
/// next item, so it reaches the caller as an error, not a panic.
pub fn execute(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    match plan {
        Plan::SeqScan { table, columns, .. } => scanned(&table, &columns, db, tx),

        index @ Plan::IndexScan { .. } => indexed(index, db, tx),
        filter @ Plan::Filter { .. } => filtered(filter, db, tx),

        Plan::Product { left, right } => join::joined(*left, *right, None, db, tx),
        Plan::HashJoin { left, right, keys, .. } => join::joined(*left, *right, Some(keys), db, tx),
        outer @ Plan::OuterJoin { .. } => join::outer(outer, db, tx),

        // [X3.135-1986 §5.22, §5.8]. Grouping must see every row, so it runs when the cursor opens.
        group @ Plan::Aggregate { .. } => grouped(group, db, tx),

        // [X3.135-1986 §5.25 GR3]
        Plan::DefaultRow { row, input } => {
            let mut rows = execute(*input, db, tx).peekable();
            if rows.peek().is_none() { Box::new(std::iter::once(Ok(row))) } else { Box::new(rows) }
        }

        // [X3.135-1986 §5.25 GR2] apply each <value expression> to each row.
        project @ Plan::Project { .. } => projected(project, db, tx),

        // [X3.135-1986 §8.3 GR2b ii] the rows of both operands.
        Plan::Append { left, right } => Box::new(execute(*left, db, tx).chain(execute(*right, db, tx))),
        set @ Plan::SetOp { .. } => set::operation(set, db, tx),
        values @ Plan::Values { .. } => set::values(values, db, tx),
        // [ISO/IEC 9075:1992 §21.1] a system table's rows, made as it is read.
        Plan::System { read } => system_rows(&read, db),

        Plan::Sort { keys, input, .. } => sorted(&keys, *input, db, tx),

        distinct @ Plan::Distinct { .. } => distinct_rows(distinct, db, tx),
    }
}

/// The rows of `table`'s columns `columns`, each read only while the
/// statement is not cancelled (RULES.md D44).
#[inline(never)]
fn scanned(table: &QualifiedName, columns: &[usize], db: &Arc<State>, tx: TxId) -> RowIter {
    match db.storage_of(table) {
        Ok(storage) => {
            let rows = Arc::clone(storage).scan(tx, columns).map(|read| read.map(|(_, row)| row));
            Box::new(checked(rows, db.tx.stop_of(tx)))
        }
        Err(e) => failed(e),
    }
}

/// The rows of the system table `read` reads, made now.
#[inline(never)]
fn system_rows(read: &SystemRead, db: &Arc<State>) -> RowIter {
    match read.rows(db) {
        Ok(rows) => Box::new(rows.into_iter().map(Ok)),
        Err(e) => failed(e),
    }
}

/// The rows the index scan `plan` finds, in index order or reversed. Never
/// inlined into `execute`, whose frame every level of a plan adds to the
/// stack (RULES.md R-CF-6), as are the helpers below.
#[inline(never)]
fn indexed(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::IndexScan { table, index, probe, reverse, .. } = plan else { return failed(broken("an index scan is not one")) };
    match probe_index(db, &table, &index, &probe, reverse) {
        Ok(Found { storage, columns, entries }) => match storage.read_by_condition(tx, &Condition { columns: columns.clone(), probe }) {
            Ok(()) => {
                let rows = entries
                    .into_iter()
                    .filter_map(move |(key, id)| fetch(&*storage, &columns, tx, &key, id).transpose().map(|r| r.map(|(_, row)| row)));
                Box::new(checked(rows, db.tx.stop_of(tx)))
            }
            Err(e) => failed(e),
        },
        Err(e) => failed(e),
    }
}

/// [X3.135-1986 §5.21 GR2] The rows of the filter `plan`.
#[inline(never)]
fn filtered(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Filter { cond, input, .. } = plan else { return failed(broken("a filter is not one")) };
    let rows = filtered_input(*input, &cond, db, tx);
    filter(cond, rows, Runner::new(db, tx))
}

/// [X3.135-1986 §5.22, §5.8] The groups of the aggregation `plan`.
#[inline(never)]
fn grouped(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    // Read by batches, if it can be (`exec/batch.rs`).
    if let Some(rows) = batch::aggregated(&plan, db, tx) {
        return rows;
    }
    let Plan::Aggregate { groups, aggs, input, .. } = plan else { return failed(broken("an aggregation is not one")) };
    let rows = execute(*input, db, tx);
    group::grouping(group::Spec { groups, aggs }, rows, db, tx)
}

/// [X3.135-1986 §8.3 GR3b] The rows of `input`, sorted by `keys`.
#[inline(never)]
fn sorted(keys: &[SortKey], input: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let rows = execute(input, db, tx);
    sort::sort(keys, rows, db)
}

/// [X3.135-1986 §5.25 GR2, GR5, §5.11 GR7] The rows of `plan`, DISTINCT:
/// duplicates per the comparison rules, nulls identical.
#[inline(never)]
fn distinct_rows(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Distinct { input } = plan else { return failed(broken("a duplicate elimination is not one")) };
    distinct::distinct_of(execute(*input, db, tx), db)
}

/// The rows of `input`, under a filter on `cond`: a scan is told the
/// condition, if it has a simple one (`simple_conjunct`).
fn filtered_input(input: Plan, cond: &Cond, db: &Arc<State>, tx: TxId) -> RowIter {
    match (input, simple_conjunct(cond)) {
        (Plan::SeqScan { table, columns, .. }, Some(condition)) => match db.storage_of(&table) {
            Ok(storage) => Box::new(Arc::clone(storage).scan_where(tx, &columns, &condition).map(|read| read.map(|(_, row)| row))),
            Err(e) => failed(e),
        },
        (input, _) => execute(input, db, tx),
    }
}

/// A conjunct of `cond` comparing a column with a constant, an equality if
/// there is one, unless `cond` can raise an error: every row `cond` keeps
/// satisfies it, so a scan tells its
/// engine (`TableStorage::scan_where`), which may then count only the rows
/// it matches as read.
fn simple_conjunct(cond: &Cond) -> Option<Condition> {
    // A condition that can raise an error must see every row: a row the
    // conjunct rejects may still raise it, and whether it does may not
    // depend on the plan, nor on which rows another transaction changed.
    if cond.can_raise() {
        return None;
    }
    let mut found = Vec::new();
    conjuncts(cond, &mut found);
    let best = found.iter().position(|(_, op, _)| *op == CompOp::Eq).unwrap_or_default();
    let (column, op, value) = found.into_iter().nth(best)?;
    Some(Condition { columns: vec![column], probe: Probe::Compare { op, value } })
}

/// The `column op constant` conjuncts of `cond`, each as the column, the
/// operator and the constant.
fn conjuncts(cond: &Cond, found: &mut Vec<(usize, CompOp, Value)>) {
    match cond {
        Cond::And(a, b) => {
            conjuncts(a, found);
            conjuncts(b, found);
        }
        Cond::Compare(Expr::Column(c), op, Expr::Const(v)) => found.push((*c, *op, v.clone())),
        // "constant op column" is "column op' constant", the operator turned around.
        Cond::Compare(Expr::Const(v), op, Expr::Column(c)) => found.push((*c, op.flip(), v.clone())),
        Cond::Compare(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::Not(_)
        | Cond::Is(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => {}
    }
}

/// What an index found: its entries, and how to read their rows.
struct Found {
    storage: Arc<dyn TableStorage>,
    /// The index's columns.
    columns: Vec<usize>,
    /// The entries, in index order or reversed.
    entries: Vec<(Key, RowId)>,
}

/// Row `id` of `storage`, as `tx` sees it, if that version has `key` in the
/// index's `columns`: an index holds the entries of versions `tx` may not
/// see (`Index`).
fn fetch(storage: &dyn TableStorage, columns: &[usize], tx: TxId, key: &Key, id: RowId) -> Result<Option<(RowId, Row)>> {
    let Some(row) = storage.get(tx, id)? else { return Ok(None) };
    let seen = project(&row, columns).map(Key);
    Ok(seen.filter(|seen| seen == key).map(|_| (id, row)))
}

/// What `index` finds for `probe`, in index order, or reversed. The rows
/// are chosen by a condition: callers tell the table's engine
/// (`TableStorage::read_by_condition`).
fn probe_index(db: &State, table: &QualifiedName, index: &str, probe: &Probe, reverse: bool) -> Result<Found> {
    let storage = Arc::clone(db.storage_of(table)?);
    let columns = db
        .catalog
        .index(index)
        .map(|ix| ix.columns.clone())
        .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no index {index}")))?;
    let mut entries = db.index_of(index)?.search(probe)?;
    if reverse {
        entries.reverse();
    }
    Ok(Found { storage, columns, entries })
}

/// [X3.135-1986 §5.21 GR2] keep the rows for which the condition is true,
/// its subqueries run by `runner`.
fn filter(cond: Cond, input: RowIter, runner: Runner) -> RowIter {
    Box::new(input.filter_map(move |r| match r.and_then(|row| Ok((eval_cond(&cond, &row, &runner)?, row))) {
        Ok((Truth::True, row)) => Some(Ok(row)),
        Ok(_) => None,
        Err(e) => Some(Err(e)),
    }))
}

/// The rows, with their ids, produced by a table access: a scan or index
/// scan, possibly filtered, possibly under a projection (which is ignored).
/// They are the object rows of a searched UPDATE or DELETE
/// [X3.135-1986 §8.12 GR1, §8.5 GR1].
/// UPDATE, DELETE and updatable cursors need the ids to change the rows.
///
/// # Errors
///
/// A row's search condition fails, or `plan` is not a table access or names
/// a table or index that does not exist (SQLCODE -910).
pub fn row_entries(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Vec<(RowId, Row)>> {
    match plan {
        Plan::Project { input, .. } => row_entries(*input, db, tx),
        Plan::SeqScan { table, columns, .. } => Arc::clone(db.storage_of(&table)?).scan(tx, &columns).collect(),
        Plan::IndexScan { table, index, probe, reverse, .. } => {
            let Found { storage, columns, entries } = probe_index(db, &table, &index, &probe, reverse)?;
            storage.read_by_condition(tx, &Condition { columns: columns.clone(), probe })?;
            entries.iter().filter_map(|(key, id)| fetch(&*storage, &columns, tx, key, *id).transpose()).collect()
        }
        Plan::Filter { cond, input, .. } => {
            let rows = match (*input, simple_conjunct(&cond)) {
                (Plan::SeqScan { table, columns, .. }, Some(condition)) => {
                    Arc::clone(db.storage_of(&table)?).scan_where(tx, &columns, &condition).collect::<Result<Vec<_>>>()?
                }
                (input, _) => row_entries(input, db, tx)?,
            };
            let mut out = Vec::new();
            let runner = Runner::new(db, tx);
            for (id, row) in rows {
                if eval_cond(&cond, &row, &runner)? == Truth::True {
                    out.push((id, row));
                }
            }
            Ok(out)
        }
        other @ (Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::DefaultRow { .. }
        | Plan::Distinct { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }) => Err(broken(&format!("not a table access: {other:?}"))),
    }
}

/// [X3.135-1986 §5.25 GR2] Each expression of the projection `plan`
/// applied to each row of its input. Never inlined into `execute`, whose
/// frame every level of a plan adds to the stack (RULES.md R-CF-6).
#[inline(never)]
fn projected(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Project { exprs, input, .. } = plan else { return failed(broken("a projection is not one")) };
    let (input, runner) = (execute(*input, db, tx), Runner::new(db, tx));
    Box::new(input.map(move |r| r.and_then(|row| exprs.iter().map(|e| eval(e, &row, &runner)).collect())))
}

/// The value of column `i` of `row`.
fn column(row: &Row, i: usize) -> Result<&Value> {
    row.get(i).ok_or_else(|| broken(&format!("column {i} is not in a row of {} columns", row.len())))
}

// Unit tests: plans the state cannot run (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::catalog::{MemoryCatalog, TableDesc};
    use crate::engine::rows::{ROW_STORE, RowStore, TableOptions};
    use crate::index::BTREE_METHOD;
    use crate::storage::StorageEngine;
    use crate::txn::NO_TRANSACTION;

    /// A state with one empty table, `A.E`, and no `A.T`.
    fn state() -> State {
        let mut st = State::with_builtins(Box::new(MemoryCatalog::default()));
        let name = QualifiedName { auth: "A".into(), ident: "E".into() };
        let desc = TableDesc {
            name: name.clone(),
            columns: vec![],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        st.storage.insert(name, Arc::from(RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap()));
        st
    }

    fn missing_table() -> QualifiedName {
        QualifiedName { auth: "A".into(), ident: "T".into() }
    }

    /// A plan of the one row `row`, read from the empty table `A.E`.
    fn one_row(row: Row) -> Plan {
        let empty = Plan::SeqScan { table: QualifiedName { auth: "A".into(), ident: "E".into() }, columns: vec![], rows: 0, cost: 0.0_f64 };
        Plan::DefaultRow { row, input: Box::new(empty) }
    }

    fn code(rows: RowIter) -> Option<Code> {
        rows.collect::<Result<Vec<_>>>().err().map(|e| e.code)
    }

    /// Verifies: [DESIGN] a plan naming a table or index the state does not
    /// have yields SQLCODE -910 rather than panicking (RULES.md R-ERR-1).
    #[test]
    fn missing_storage_and_indexes_are_reported() {
        let st = Arc::new(state());
        let seq = || Plan::SeqScan { table: missing_table(), columns: vec![], rows: 0, cost: 0.0_f64 };
        let index = || Plan::IndexScan {
            table: missing_table(),
            index: "I".into(),
            method: BTREE_METHOD.into(),
            probe: Probe::All,
            reverse: false,
            detail: String::new(),
            est: 0.0_f64,
            cost: 0.0_f64,
        };
        assert_eq!(code(execute(seq(), &st, NO_TRANSACTION)), Some(Code::Api));
        assert_eq!(code(execute(index(), &st, NO_TRANSACTION)), Some(Code::Api));
        assert_eq!(row_entries(seq(), &st, NO_TRANSACTION).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(row_entries(index(), &st, NO_TRANSACTION).err().map(|e| e.code), Some(Code::Api));
        let product = Plan::Product { left: Box::new(seq()), right: Box::new(seq()) };
        assert_eq!(code(execute(product, &st, NO_TRANSACTION)), Some(Code::Api));
    }

    /// Verifies: [DESIGN] `row_entries` refuses a plan that is not a table
    /// access with SQLCODE -910.
    #[test]
    fn row_entries_needs_a_table_access() {
        let st = Arc::new(state());
        let plan = Plan::Distinct { input: Box::new(one_row(vec![])) };
        assert_eq!(row_entries(plan, &st, NO_TRANSACTION).err().map(|e| e.code), Some(Code::Api));
    }

    /// Verifies: [DESIGN] a column the row does not have, in an expression, a
    /// null predicate, a grouping or a sort key, is SQLCODE -910, not a panic.
    #[test]
    fn columns_outside_the_row_are_reported() {
        let st = Arc::new(state());
        let row = vec![Value::Null];
        assert_eq!(eval(&Expr::Column(1), &row, &NoSubqueries).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(eval_cond(&Cond::IsNull(Expr::Column(1), false), &row, &NoSubqueries).err().map(|e| e.code), Some(Code::Api));
        let aggregate = Plan::Aggregate { groups: vec![1], aggs: vec![], detail: String::new(), input: Box::new(one_row(row.clone())) };
        assert_eq!(code(execute(aggregate, &st, NO_TRANSACTION)), Some(Code::Api));
        let keys = vec![SortKey { column: 1, descending: false }];
        let sort = Plan::Sort { keys, detail: String::new(), input: Box::new(one_row(vec![Value::Null])) };
        assert_eq!(code(execute(sort, &st, NO_TRANSACTION)), Some(Code::Api));
        assert_eq!(code(execute(one_row(row), &st, NO_TRANSACTION)), None, "the row source itself succeeds");
    }
}
