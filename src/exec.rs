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
//! INTERSECT, subqueries' rows) hold up to their work grants of them in memory
//! and spill the rest (`exec/work.rs`, RULES.md D38); so do the rows a
//! statement holds past them, a cursor's or a write's (`exec/held.rs`).

mod answered;
mod batch;
mod distinct;
mod entries;
mod eval;
mod graph_table;
mod group;
mod held;
mod invoked;
mod join;
mod json_table;
mod limit;
mod merged;
mod nearest;
mod parallel;
mod partition;
mod pattern;
mod program;
mod project;
mod ptf;
mod read;
mod recursive;
mod scan;
mod set;
mod sort;
mod subquery;
mod transition;
mod unnest;
mod window;
mod work;

use std::sync::Arc;

use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::{Cond, Expr, SortKey, Truth};
use crate::physical::Plan;
use crate::storage::Row;
use crate::system::SystemRead;
use crate::txn::TxId;
use crate::value::Value;

pub use entries::{Entries, row_entries, table_entries};
pub(crate) use eval::{Escape, like_escape};
pub use eval::{Heard, eval, eval_cond, quantified, warn};
pub use held::{HeldIter, HeldRows, Holdable};
pub use join::{DEFAULT_PARTITION_DEPTH, DEFAULT_PARTITIONS};
pub use recursive::{DEFAULT_MAX_ITERATIONS, WorkRows};
pub use sort::compare_rows;
pub use transition::TransitionRows;

/// [X3.135-1986 §8.3 GR3b] `rows` sorted by `keys`, in a work area of
/// `db` that spills (RULES.md D38), for the statement of `tx`.
pub fn sort_rows(keys: &[SortKey], rows: RowIter, db: &State, tx: TxId) -> RowIter {
    sort::sort(keys, rows, db, tx)
}
pub(crate) use program::{Ended, Host, rows, run_program};
pub use subquery::{DEFAULT_DECORRELATE_AFTER, Drawn, NoSubqueries, Runner, Subqueries, SubqueryRows};

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

/// A function giving the rows of a plan of one kind, read as a
/// transaction sees them.
type Opened = fn(Plan, &Arc<State>, TxId) -> RowIter;

/// Rows are read as transaction `tx` sees them. A failure is the iterator's
/// next item, so it reaches the caller as an error, not a panic.
pub fn execute(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    // Matched by reference, the function chosen then handed the plan
    // whole at one call, so no arm adds a plan of its own, nor a copy of
    // one for its call, to the frame every level of a plan adds to the
    // stack (RULES.md R-CF-6).
    let run: Opened = match &plan {
        Plan::SeqScan { .. } => scan::scanned_plan,
        Plan::IndexScan { .. } => scan::indexed,
        Plan::Nearest(_) => nearest::nearest,
        Plan::Filter { .. } => filtered,
        Plan::Product { .. } | Plan::HashJoin { .. } => joined_plan,
        Plan::OuterJoin { .. } => join::outer,
        Plan::IndexJoin { .. } => join::indexed,
        // [X3.135-1986 §5.22, §5.8]. Grouping must see every row, so it runs when the cursor opens.
        Plan::Aggregate { .. } => grouped,
        // [X3.135-1986 §5.25 GR2] apply each <value expression> to each row.
        Plan::Project { .. } => projected,
        Plan::Append { .. } => appended,
        Plan::MergeAppend { .. } => merged::merged,
        Plan::SetOp { .. } => set::operation,
        Plan::Values { .. } => set::values,
        Plan::System { .. } => system_plan,
        Plan::Sort { .. } => sorted_plan,
        // [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] its counts evaluated as it opens.
        Plan::Limit { .. } => limit::limited,
        Plan::Distinct { .. } => distinct_rows,
        // [ISO/IEC 9075-2:2023 §7.17 GR2c ii] its fixpoint is found as the cursor opens.
        Plan::Recursive { .. } => recursive::recursive,
        Plan::WorkTable { .. } => recursive::work_table,
        // [ISO/IEC 9075-2:2023 §15.21 GR5a] the state change's rows, as given.
        Plan::Transition { .. } => transition::transition,
        // [ISO/IEC 9075-2:2023 §7.6 SR8] each row of its input with its collections' elements.
        // [ISO/IEC 9075-2:2023 §7.11] or a JSON table's rows of each.
        Plan::Unnest(u) => {
            // [GS §8] or a GRAPH_TABLE's rows of each.
            if u.graph.is_some() {
                graph_table::graph_table
            } else if u.json.is_some() {
                json_table::json_table
            } else {
                unnest::unnested
            }
        }
        // [ISO/IEC 9075-2:2023 §7.15] each row of its input with its window functions' values.
        Plan::Window { .. } => window::windowed,
        Plan::Recognize { .. } => pattern::recognized,
        // [ISO/IEC 9075-2:2023 §9.25] its component procedures run as it opens.
        Plan::Polymorphic(..) => ptf::polymorphic,
        // [DESIGN] by morsels on the workers (RULES.md D290).
        Plan::Parallel(..) => parallel::parallel,
    };
    run(plan, db, tx)
}

/// The rows of the product or hash join `plan`.
#[inline(never)]
fn joined_plan(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    match plan {
        Plan::Product { left, right } => join::joined(*left, *right, None, db, tx),
        Plan::HashJoin { left, right, keys, .. } => join::joined(*left, *right, Some(keys), db, tx),
        Plan::SeqScan { .. }
        | Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Recognize { .. }
        | Plan::Polymorphic(..)
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..) => failed(broken("a join is not one")),
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
fn system_plan(plan: Plan, db: &Arc<State>, _tx: TxId) -> RowIter {
    let Plan::System { read } = plan else { return failed(broken("a system table's read is not one")) };
    system_rows(&read, db)
}

/// The rows of the sort `plan`: of its first alone asked for, those.
#[inline(never)]
fn sorted_plan(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Sort { keys, input, most, .. } = plan else { return failed(broken("a sort is not one")) };
    match most {
        Some(most) => sort::first(&keys, execute(*input, db, tx), most, db, tx),
        None => sorted(&keys, *input, db, tx),
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

/// [X3.135-1986 §5.21 GR2] The rows of the filter `plan`.
#[inline(never)]
fn filtered(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Filter { cond, input, .. } = plan else { return failed(broken("a filter is not one")) };
    let rows = scan::filtered_input(*input, &cond, db, tx);
    filter(cond, rows, Runner::new(db, tx))
}

/// [X3.135-1986 §5.22, §5.8] The groups of the aggregation `plan`.
#[inline(never)]
fn grouped(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    // Read by batches, if it can be (`exec/batch.rs`).
    if batched(db) && batch::aggregable(&plan, db) {
        return batch::aggregated(plan, db, tx);
    }
    let Plan::Aggregate { groups, collations, aggs, seeds, input, .. } = plan else { return failed(broken("an aggregation is not one")) };
    let rows = execute(*input, db, tx);
    group::grouping(group::Spec { groups, collations, aggs, seeds }, rows, db, tx)
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
    if let Some(columns) = project::Columns::of(&exprs) {
        if matches!(*input, Plan::IndexJoin { .. }) {
            return join::indexed_projected(*input, Some(columns), db, tx);
        }
        let input = execute(*input, db, tx);
        return Box::new(input.map(move |row| row.and_then(|row| columns.take(row))));
    }
    let (input, runner) = (execute(*input, db, tx), Runner::new(db, tx));
    // [ISO/IEC 9075-2:2023 §4.29.1] a row's draws of one generator are one.
    if exprs.iter().any(Expr::draws) {
        return Box::new(input.map(move |r| {
            let drawn = Drawn::new(&runner);
            r.and_then(|row| exprs.iter().map(|e| eval(e, &row, &drawn)).collect())
        }));
    }
    Box::new(input.map(move |r| r.and_then(|row| exprs.iter().map(|e| eval(e, &row, &runner)).collect())))
}

/// Whether a query reads its tables a batch at a time where it can, as the
/// setting EXECUTOR.BATCHES says.
pub(crate) fn batched(db: &State) -> bool {
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

    use crate::catalog::{Catalog, MemoryCatalog, QualifiedName, TableDesc};
    use crate::engine::rows::{ROW_STORE, RowStore, TableOptions};
    use crate::index::{BTREE_METHOD, Probe};
    use crate::storage::StorageEngine;
    use crate::txn::NO_TRANSACTION;

    /// A state with one empty table, `A.E`, of no columns, and no `A.T`.
    pub(super) fn state() -> State {
        let desc = state_desc();
        let mut catalog = MemoryCatalog::default();
        catalog.add_table(desc.clone());
        let mut st = State::with_builtins(Box::new(catalog));
        st.storage
            .insert(Arc::new(desc.name.clone()), Arc::from(RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap()));
        st
    }

    fn missing_table() -> QualifiedName {
        QualifiedName { auth: "A".into(), ident: "T".into() }
    }

    /// A plan of the one row `row`, a table value constructor of its values.
    fn one_row(row: Row) -> Plan {
        Plan::Values { rows: vec![row.into_iter().map(Expr::Const).collect()], detail: String::new() }
    }

    fn code(rows: RowIter) -> Option<Code> {
        rows.collect::<Result<Vec<_>>>().err().map(|e| e.code)
    }

    /// Verifies: [DESIGN] a plan naming a table or index the state does not
    /// have yields SQLCODE -910 rather than panicking (RULES.md R-ERR-1).
    #[test]
    fn missing_storage_and_indexes_are_reported() {
        let st = Arc::new(state());
        let seq = || Plan::SeqScan {
            table: missing_table(),
            columns: vec![],
            rows: 0,
            cost: 0.0_f64,
            order: vec![],
            backward: false,
            most: None,
            located: false,
        };
        let index = || Plan::IndexScan {
            table: missing_table(),
            index: "I".into(),
            method: BTREE_METHOD.into(),
            probe: Probe::All,
            slots: Vec::new(),
            reverse: false,
            columns: Vec::new(),
            detail: String::new(),
            est: 0.0_f64,
            cost: 0.0_f64,
            covering: false,
            located: false,
            decided: None,
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
            let column =
                ColumnDesc { name: "K".into(), data_type, not_null: false, default: None, domain: None, collation: None, generated: None };
            let desc = TableDesc { name: name.clone(), columns: vec![column], ..state_desc() };
            let table = RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap();
            for k in 0..ROWS {
                table.insert(NO_TRANSACTION, vec![Value::Exact(Decimal::new(ROWS - k, 0))]).unwrap();
            }
            st.storage.insert(Arc::new(name.clone()), Arc::from(table));
            let plan = Plan::SeqScan {
                table: name,
                columns: vec![0],
                rows: 0,
                cost: 0.0_f64,
                order: vec![0],
                backward: false,
                most: None,
                located: false,
            };
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
            periods: Vec::new(),
            typed: None,
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
        assert_eq!(eval(&Expr::Column(1), &row, &NoSubqueries).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(eval_cond(&Cond::IsNull(Expr::Column(1), false), &row, &NoSubqueries).err().map(|e| e.code), Some(Code::Api));
        let aggregate = Plan::Aggregate {
            groups: vec![1],
            collations: vec![],
            aggs: vec![],
            seeds: vec![],
            detail: String::new(),
            input: Box::new(one_row(row.clone())),
        };
        assert_eq!(code(execute(aggregate, &st, NO_TRANSACTION)), Some(Code::Api));
        let keys = vec![SortKey::ascending(1)];
        let sort = Plan::Sort { keys, detail: String::new(), input: Box::new(one_row(vec![Value::Null])), most: None };
        assert_eq!(code(execute(sort, &st, NO_TRANSACTION)), Some(Code::Api));
        assert_eq!(code(execute(one_row(row), &st, NO_TRANSACTION)), None, "the row source itself succeeds");
    }

    /// Verifies: [DESIGN] each executor of one kind of plan, given a plan
    /// of another, yields SQLCODE -910 rather than panicking (RULES.md
    /// R-ERR-1); and the batch reader takes it for none of its own.
    #[test]
    fn executors_refuse_a_plan_of_another_kind() {
        let st = Arc::new(state());
        let other = || one_row(vec![Value::Null]);
        let executors: [Opened; 10] = [
            scan::scanned_plan,
            appended,
            sorted_plan,
            scan::indexed,
            filtered,
            grouped,
            distinct_rows,
            projected,
            set::operation,
            system_plan,
        ];
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
        let cut = Expr::Cast(Box::new(Expr::Const(Value::Char("ABC".into()))), DataType::character(1));
        assert_eq!(eval(&cut, &Vec::new(), &NoSubqueries).unwrap().to_string(), "A");
        let warnings = Warnings::default();
        assert_eq!(eval(&cut, &Vec::new(), &Heard(&warnings)).unwrap().to_string(), "A");
        assert_eq!(warnings.take(), Some(SqlState::STRING_TRUNCATED));
        let plan = LogicalPlan::Scan { table: missing_table(), names: vec![], needed: vec![], located: false };
        let scalar = Expr::Subquery(Arc::new(Subquery::new(plan, vec![])), 0);
        assert_eq!(eval(&scalar, &Vec::new(), &Heard(&warnings)).err().map(|e| e.code), Some(Code::Api));
    }
}
