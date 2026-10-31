//! Execution of physical plans: the General Rules that produce rows. The FROM
//! product [X3.135-1986 §5.20 GR2b], WHERE and HAVING [X3.135-1986 §5.21 GR2,
//! §5.23], grouping and set functions [X3.135-1986 §5.22, §5.8], the select
//! list and DISTINCT [X3.135-1986 §5.25 GR2, GR3], UNION and ORDER BY
//! [X3.135-1986 §8.3 GR2b, GR3b], EXCEPT, INTERSECT and table value
//! constructors [ISO/IEC 9075:1992 §7.10, §7.2], with three-valued logic
//! [X3.135-1986 §5.18].
//!
//! \[DESIGN\] Each plan node becomes an iterator over rows (the Volcano /
//! iterator model). Rows are pulled one at a time from the top; an
//! aggregation, projection, sort, EXCEPT or INTERSECT of tables' scans
//! reads them a batch at a time where it can (`exec/batch.rs`,
//! EXECUTOR.BATCHES). Operators that must
//! hold rows (sorts, grouping, DISTINCT, joins and products, EXCEPT and
//! INTERSECT, subqueries' rows) hold up to MEMORY.WORK_BYTES of them in memory
//! and spill the rest (`exec/work.rs`, RULES.md D38); so do the rows a
//! statement holds past them, a cursor's or a write's (`exec/held.rs`).

mod batch;
mod distinct;
mod entries;
mod eval;
mod group;
mod held;
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
use crate::index::{Entries as IndexEntries, Key, KeyOrder, Probe};
use crate::logical::{Cond, SortKey, Truth, pushed_conjuncts};
use crate::physical::Plan;
use crate::storage::{Condition, Conjunction, Row, RowId, TableStorage, project};
use crate::system::SystemRead;
use crate::txn::{TxId, checked};
use crate::value::Value;

pub use entries::{Entries, row_entries, table_entries};
pub use eval::{Heard, eval, eval_cond, quantified, warn};
pub use held::{HeldIter, HeldRows, Holdable};
pub use join::{DEFAULT_PARTITION_DEPTH, DEFAULT_PARTITIONS};
pub use sort::compare_rows;

/// [X3.135-1986 §8.3 GR3b] `rows` sorted by `keys`, in a work area of
/// `db` that spills (RULES.md D38), for the statement of `tx`.
pub fn sort_rows(keys: &[SortKey], rows: RowIter, db: &State, tx: TxId) -> RowIter {
    sort::sort(keys, rows, db, tx)
}
pub use subquery::{DEFAULT_DECORRELATE_AFTER, NoSubqueries, Runner, Subqueries, SubqueryRows};

/// The rows a plan produces, each read or computed when it is pulled. The
/// iterator owns what it reads (the state, through an `Arc`, and the
/// tables' scans), so it can be kept between calls: a cursor pulls rows as
/// it is fetched from.
pub type RowIter = Box<dyn Iterator<Item = Result<Row>> + Send>;

/// \[DESIGN\] A broken invariant between the planner and the executor: a plan
/// that names something the state does not have. SQLCODE -910, never a panic
/// (RULES.md R-ERR-1).
pub(in crate::exec) fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what}"))
}

/// The rows of a plan that failed as it opened: just its error.
fn failed(e: SqlError) -> RowIter {
    Box::new(std::iter::once(Err(e)))
}

/// Rows are read as transaction `tx` sees them. A failure is the iterator's
/// next item, so it reaches the caller as an error, not a panic.
pub fn execute(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    // Matched by reference, the plan handed whole to each arm's function,
    // so no arm adds a plan of its own to the frame every level of a plan
    // adds to the stack (RULES.md R-CF-6).
    match &plan {
        Plan::SeqScan { .. } => scanned_plan(plan, db, tx),
        Plan::IndexScan { .. } => indexed(plan, db, tx),
        Plan::Filter { .. } => filtered(plan, db, tx),
        Plan::Product { .. } | Plan::HashJoin { .. } => joined_plan(plan, db, tx),
        Plan::OuterJoin { .. } => join::outer(plan, db, tx),
        Plan::IndexJoin { .. } => join::indexed(plan, db, tx),
        // [X3.135-1986 §5.22, §5.8]. Grouping must see every row, so it runs when the cursor opens.
        Plan::Aggregate { .. } => grouped(plan, db, tx),
        // [X3.135-1986 §5.25 GR2] apply each <value expression> to each row.
        Plan::Project { .. } => projected(plan, db, tx),
        Plan::Append { .. } => appended(plan, db, tx),
        Plan::SetOp { .. } => set::operation(plan, db, tx),
        Plan::Values { .. } => set::values(plan, db, tx),
        Plan::System { .. } => system_plan(plan, db),
        Plan::Sort { .. } => sorted_plan(plan, db, tx),
        Plan::Distinct { .. } => distinct_rows(plan, db, tx),
    }
}

/// The rows of the scan `plan`.
#[inline(never)]
fn scanned_plan(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::SeqScan { table, columns, order, .. } = plan else { return failed(broken("a scan is not one")) };
    if order.is_empty() {
        return scanned(&table, &columns, db, tx);
    }
    ordered_scan((&table, &columns, &order), None, db, tx)
}

/// The rows of `table`, `columns` read, in the order of the columns
/// `order`, as ORDER BY sorts them [X3.135-1986 §8.3 GR3b], only those
/// `condition` may match if given: as its storage reads them so
/// (`TableStorage::scan_ordered`), else read and sorted, if it no longer
/// can, its option changed since the plan was made; each row read only
/// while the statement is not cancelled (RULES.md D44).
fn ordered_scan(
    (table, columns, order): (&QualifiedName, &[usize], &[usize]),
    condition: Option<&Conjunction>,
    db: &Arc<State>,
    tx: TxId,
) -> RowIter {
    let storage = match db.storage_of(table) {
        Ok(storage) => Arc::clone(storage),
        Err(e) => return failed(e),
    };
    let stop = db.tx.stop_of(tx);
    if let Some(rows) = Arc::clone(&storage).scan_ordered(tx, columns, order, condition) {
        return Box::new(checked(rows.map(|read| read.map(|(_, row)| row)), stop));
    }
    let read = match condition {
        Some(c) => storage.scan_where(tx, columns, c),
        None => storage.scan(tx, columns),
    };
    // Sorted as ORDER BY sorts, in a work area that spills (RULES.md D38).
    let keys: Vec<SortKey> = order.iter().map(|&column| SortKey { column, descending: false }).collect();
    sort::sort(&keys, Box::new(checked(read.map(|r| r.map(|(_, row)| row)), stop)), db, tx)
}

/// The rows of the product or hash join `plan`.
#[inline(never)]
fn joined_plan(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    match plan {
        Plan::Product { left, right } => join::joined(*left, *right, None, db, tx),
        Plan::HashJoin { left, right, keys, .. } => join::joined(*left, *right, Some(keys), db, tx),
        Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => failed(broken("a join is not one")),
    }
}

/// [X3.135-1986 §8.3 GR2b ii] the rows of both operands of `plan`.
#[inline(never)]
fn appended(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Append { left, right } = plan else { return failed(broken("an append is not one")) };
    Box::new(execute(*left, db, tx).chain(execute(*right, db, tx)))
}

/// [ISO/IEC 9075:1992 §21.1] a system table's rows, made as it is read.
#[inline(never)]
fn system_plan(plan: Plan, db: &Arc<State>) -> RowIter {
    let Plan::System { read } = plan else { return failed(broken("a system table's read is not one")) };
    system_rows(&read, db)
}

/// The rows of the sort `plan`.
#[inline(never)]
fn sorted_plan(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Sort { keys, input, .. } = plan else { return failed(broken("a sort is not one")) };
    sorted(&keys, *input, db, tx)
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
                let rows = checked(entries, db.tx.stop_of(tx)).filter_map(move |entry| {
                    entry.and_then(|(key, id)| fetch(&*storage, &columns, tx, &key, id)).transpose().map(|r| r.map(|(_, row)| row))
                });
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
    if batched(db) && batch::aggregable(&plan, db) {
        return batch::aggregated(plan, db, tx);
    }
    let Plan::Aggregate { groups, aggs, input, .. } = plan else { return failed(broken("an aggregation is not one")) };
    let rows = execute(*input, db, tx);
    group::grouping(group::Spec { groups, aggs }, rows, db, tx)
}

/// [X3.135-1986 §8.3 GR3b] The rows of `input`, sorted by `keys`.
#[inline(never)]
fn sorted(keys: &[SortKey], input: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    // A stream's batches sorted as they are, if it can be (`sort/vectors.rs`).
    if batched(db) && batch::whole_width(&input, db).is_some() {
        return batch::kept(input, db, tx).map_or_else(failed, |batches| sort::sort_batches(keys, batches, db, tx));
    }
    let rows = execute(input, db, tx);
    sort::sort(keys, rows, db, tx)
}

/// [X3.135-1986 §5.25 GR2, GR5, §5.11 GR7] The rows of `plan`, DISTINCT:
/// duplicates per the comparison rules, nulls identical.
#[inline(never)]
fn distinct_rows(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    // Grouped by batches, if it can be (`batch::distinct_grouping`).
    if let Some(width) = batched(db).then(|| batch::distinct_groupable(&plan, db)).flatten() {
        return grouped(batch::distinct_grouping(plan, width), db, tx);
    }
    let Plan::Distinct { input } = plan else { return failed(broken("a duplicate elimination is not one")) };
    distinct::distinct_of(execute(*input, db, tx), db, tx)
}

/// The rows of `input`, under a filter on `cond`: a scan is told the
/// conditions, if it has simple ones (`logical::pushed_conjuncts`), each
/// row read only while the statement is not cancelled (RULES.md D44).
fn filtered_input(input: Plan, cond: &Cond, db: &Arc<State>, tx: TxId) -> RowIter {
    match (input, pushed_conjuncts(cond)) {
        (Plan::SeqScan { table, columns, order, .. }, Some(condition)) => {
            if !order.is_empty() {
                return ordered_scan((&table, &columns, &order), Some(&condition), db, tx);
            }
            match db.storage_of(&table) {
                Ok(storage) => {
                    let rows = Arc::clone(storage).scan_where(tx, &columns, &condition).map(|read| read.map(|(_, row)| row));
                    Box::new(checked(rows, db.tx.stop_of(tx)))
                }
                Err(e) => failed(e),
            }
        }
        (input, _) => execute(input, db, tx),
    }
}

/// What an index found: its entries, and how to read their rows.
struct Found {
    storage: Arc<dyn TableStorage>,
    /// The index's columns.
    columns: Vec<usize>,
    /// The entries, in index order or reversed, read as they are taken.
    entries: IndexEntries,
}

/// Row `id` of `storage`, as `tx` sees it, if that version has `key` in the
/// index's `columns`: an index holds the entries of versions `tx` may not
/// see (`Index`).
fn fetch(storage: &dyn TableStorage, columns: &[usize], tx: TxId, key: &Key, id: RowId) -> Result<Option<(RowId, Row)>> {
    let Some(row) = storage.get(tx, id)? else { return Ok(None) };
    let seen = project(&row, columns).map(Key);
    Ok(seen.filter(|seen| seen == key).map(|_| (id, row)))
}

/// What `index` finds for `probe`, in index order, or reversed, read as
/// it is taken: callers look for a cancel as each entry is read (RULES.md
/// D44). The rows are chosen by a condition: callers tell the table's
/// engine (`TableStorage::read_by_condition`).
fn probe_index(db: &State, table: &QualifiedName, index: &str, probe: &Probe, reverse: bool) -> Result<Found> {
    let storage = Arc::clone(db.storage_of(table)?);
    let columns = db
        .catalog
        .index(index)
        .map(|ix| ix.columns.clone())
        .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no index {index}")))?;
    let order = if reverse { KeyOrder::Descending } else { KeyOrder::Ascending };
    let entries = Arc::clone(db.index_of(index)?).search(probe, order);
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

/// [X3.135-1986 §5.25 GR2] Each expression of the projection `plan`
/// applied to each row of its input. Never inlined into `execute`, whose
/// frame every level of a plan adds to the stack (RULES.md R-CF-6).
#[inline(never)]
fn projected(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    // Read by batches, if it can be (`exec/batch/project.rs`).
    if batched(db) && batch::projectable(&plan, db) {
        return batch::projected(plan, db, tx);
    }
    let Plan::Project { exprs, input, .. } = plan else { return failed(broken("a projection is not one")) };
    let (input, runner) = (execute(*input, db, tx), Runner::new(db, tx));
    Box::new(input.map(move |r| r.and_then(|row| exprs.iter().map(|e| eval(e, &row, &runner)).collect())))
}

/// Whether a query reads its tables a batch at a time where it can, as the
/// setting EXECUTOR.BATCHES says.
pub(super) fn batched(db: &State) -> bool {
    if let Value::Char(s) = db.settings.get(crate::settings::EXECUTOR_BATCHES) { s != "OFF" } else { true }
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
        let desc = state_desc();
        st.storage.insert(desc.name.clone(), Arc::from(RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap()));
        st
    }

    fn missing_table() -> QualifiedName {
        QualifiedName { auth: "A".into(), ident: "T".into() }
    }

    /// A plan of the one row `row`, a table value constructor of its values.
    fn one_row(row: Row) -> Plan {
        Plan::Values { rows: vec![row.into_iter().map(crate::logical::Expr::Const).collect()], detail: String::new() }
    }

    fn code(rows: RowIter) -> Option<Code> {
        rows.collect::<Result<Vec<_>>>().err().map(|e| e.code)
    }

    /// Verifies: [DESIGN] a plan naming a table or index the state does not
    /// have yields SQLCODE -910 rather than panicking (RULES.md R-ERR-1).
    #[test]
    fn missing_storage_and_indexes_are_reported() {
        let st = Arc::new(state());
        let seq = || Plan::SeqScan { table: missing_table(), columns: vec![], rows: 0, cost: 0.0_f64, order: vec![] };
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

    /// Verifies: [DESIGN] RULES.md D38, D31 [X3.135-1986 §8.3 GR3b]: a scan
    /// planned in an order its table no longer gives is read and sorted in
    /// a work area that spills: with MEMORY.WORK_BYTES at its least, its rows come
    /// in order; runs that cannot be read back fail it (SQLCODE -915), so
    /// its rows were written to them.
    #[test]
    fn a_scan_no_longer_in_order_is_sorted_by_spilling() {
        use crate::catalog::ColumnDesc;
        use crate::spill::testing::{Held, areas};
        use crate::value::{DataType, Decimal, Spelled};
        const ROWS: i128 = 5000;
        for broken in [false, true] {
            let mut st = state();
            st.settings.set("MEMORY.WORK_BYTES", Some(&Value::Exact(Decimal::new(64 << 10, 0)))).unwrap();
            st.work = areas(Some(Arc::new(Held { broken })), usize::MAX);
            let name = QualifiedName { auth: "A".into(), ident: "N".into() };
            let data_type = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
            let column = ColumnDesc { name: "K".into(), data_type, not_null: false, default: None, domain: None };
            let desc = TableDesc { name: name.clone(), columns: vec![column], ..state_desc() };
            let table = RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap();
            for k in 0..ROWS {
                table.insert(NO_TRANSACTION, vec![Value::Exact(Decimal::new(ROWS - k, 0))]).unwrap();
            }
            st.storage.insert(name.clone(), Arc::from(table));
            let plan = Plan::SeqScan { table: name, columns: vec![0], rows: 0, cost: 0.0_f64, order: vec![0] };
            let read = execute(plan, &Arc::new(st), NO_TRANSACTION).collect::<Result<Vec<Row>>>();
            if broken {
                assert_eq!(read.err().map(|e| e.code), Some(Code::Io), "spilled");
                continue;
            }
            let keys: Vec<String> = read.unwrap().iter().map(|r| r.iter().map(ToString::to_string).collect()).collect();
            assert_eq!(keys, (1..=ROWS).map(|k| k.to_string()).collect::<Vec<_>>());
        }
    }

    /// The description of `A.E`, of no columns.
    fn state_desc() -> TableDesc {
        TableDesc {
            name: QualifiedName { auth: "A".into(), ident: "E".into() },
            columns: vec![],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        }
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
        assert_eq!(eval(&crate::logical::Expr::Column(1), &row, &NoSubqueries).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(
            eval_cond(&Cond::IsNull(crate::logical::Expr::Column(1), false), &row, &NoSubqueries).err().map(|e| e.code),
            Some(Code::Api)
        );
        let aggregate = Plan::Aggregate { groups: vec![1], aggs: vec![], detail: String::new(), input: Box::new(one_row(row.clone())) };
        assert_eq!(code(execute(aggregate, &st, NO_TRANSACTION)), Some(Code::Api));
        let keys = vec![SortKey { column: 1, descending: false }];
        let sort = Plan::Sort { keys, detail: String::new(), input: Box::new(one_row(vec![Value::Null])) };
        assert_eq!(code(execute(sort, &st, NO_TRANSACTION)), Some(Code::Api));
        assert_eq!(code(execute(one_row(row), &st, NO_TRANSACTION)), None, "the row source itself succeeds");
    }

    /// An executor of one kind of plan.
    type Executor = fn(Plan, &Arc<State>, TxId) -> RowIter;

    /// Verifies: [DESIGN] each executor of one kind of plan, given a plan
    /// of another, yields SQLCODE -910 rather than panicking (RULES.md
    /// R-ERR-1); and the batch reader takes it for none of its own.
    #[test]
    fn executors_refuse_a_plan_of_another_kind() {
        let st = Arc::new(state());
        let other = || one_row(vec![Value::Null]);
        let executors: [Executor; 10] =
            [scanned_plan, appended, sorted_plan, indexed, filtered, grouped, distinct_rows, projected, set::operation, |plan, db, _| {
                system_plan(plan, db)
            }];
        for run in executors {
            assert_eq!(code(run(other(), &st, NO_TRANSACTION)), Some(Code::Api));
        }
        let distinct = Plan::Distinct { input: Box::new(other()) };
        assert_eq!(code(set::values(distinct, &st, NO_TRANSACTION)), Some(Code::Api));
        assert!(!batch::aggregable(&other(), &st) && batch::distinct_groupable(&other(), &st).is_none());
        assert!(matches!(batch::distinct_grouping(other(), 1_usize), Plan::Values { .. }), "not a DISTINCT: left as it is");
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §3.3.4.1, §6.10 GR5c ii]
    /// where no subquery can be, a warning an expression raises is heard
    /// only by what holds `Warnings` (`Heard`), and lost to `NoSubqueries`;
    /// neither runs a subquery (SQLCODE -910).
    #[test]
    fn warnings_are_heard_where_they_are_held() {
        use crate::error::{SqlState, Warnings};
        use crate::logical::{Expr, LogicalPlan, Subquery};
        use crate::value::DataType;
        let cut = Expr::Cast(Box::new(Expr::Const(Value::Char("ABC".into()))), DataType::Character { length: 1 });
        assert_eq!(eval(&cut, &Vec::new(), &NoSubqueries).unwrap().to_string(), "A");
        let warnings = Warnings::default();
        assert_eq!(eval(&cut, &Vec::new(), &Heard(&warnings)).unwrap().to_string(), "A");
        assert_eq!(warnings.take(), Some(SqlState::STRING_TRUNCATED));
        let plan = LogicalPlan::Scan { table: missing_table(), names: vec![], needed: vec![] };
        let scalar = Expr::Subquery(Arc::new(Subquery { plan, outer: vec![] }), 0);
        assert_eq!(eval(&scalar, &Vec::new(), &Heard(&warnings)).err().map(|e| e.code), Some(Code::Api));
    }
}
