//! \[DESIGN\] Physical planning: decide *how* to compute a logical plan.
//!
//! Nothing about storage is assumed here. Every number comes from a plugin:
//!   * the storage engine prices scans and row fetches (`TableStorage`),
//!   * each index estimates how many rows a probe returns (`Index`),
//!   * the storage engine's statistics estimate how many rows a condition
//!     keeps (`Statistics`, `physical/estimate.rs`),
//!   * each index method declares which operators it answers and whether its
//!     output is in key order (`IndexMethod`),
//!   * the remaining weights live in a `CostModel` the user can change.
//!
//! "Interesting orders" (System R: Selinger et al., "Access Path Selection in a
//! Relational Database Management System", SIGMOD 1979): when a Sort is
//! required, the order is passed down through operators that preserve it.
//! A table access may then choose an ordered index path, and the Sort is
//! dropped if that is cheaper than scanning in any order and sorting.

mod access;
mod cost;
mod estimate;
mod join;
mod order;

pub use cost::{CostModel, DEFAULT_EQUAL, DEFAULT_INDEX_DESCENT, DEFAULT_NULL, DEFAULT_RANGE, DEFAULT_REORDER_GAIN, DEFAULT_SORT_COMPARE};

use std::fmt::Write;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::index::Probe;
use crate::logical::{
    AggCall, Cond, Expr, LogicalPlan, OuterJoin, SetOp, SortKey, column_name, render_cond, render_expr, render_keys, split_and,
};
use crate::storage::Row;
use crate::system::SystemRead;

/// \[DESIGN\] A physical plan: the operators the executor runs, each with what
/// EXPLAIN shows of it.
#[derive(Debug)]
pub enum Plan {
    /// Every row of a table, in the engine's order.
    SeqScan {
        /// The table read.
        table: QualifiedName,
        /// The columns the statement uses: the only ones the engine need read.
        columns: Vec<usize>,
        /// Its row count when planned.
        rows: usize,
        /// The planner's cost estimate.
        cost: f64,
        /// The columns, by their places, in whose order, ascending, it is read
        /// (`TableStorage::scan_ordered`); none, in no particular order.
        order: Vec<usize>,
    },
    /// The rows of a table an index finds for a probe, in index order.
    IndexScan {
        /// The table read.
        table: QualifiedName,
        /// The index probed.
        index: String,
        /// The index's method, for EXPLAIN.
        method: String,
        /// What is looked up.
        probe: Probe,
        /// Read the index backwards (for ORDER BY ... DESC).
        reverse: bool,
        /// The condition the probe answers, as text.
        detail: String,
        /// The estimate of the rows found: the index's, or else from the
        /// table's statistics.
        est: f64,
        /// The planner's cost estimate.
        cost: f64,
    },
    /// The rows of `input` for which `cond` is true [X3.135-1986 §5.21 GR2].
    Filter {
        /// The condition.
        cond: Cond,
        /// The condition, as text.
        detail: String,
        /// The rows it is estimated to keep, if its input is a table.
        est: Option<f64>,
        /// The rows filtered.
        input: Box<Plan>,
    },
    /// Every row of `left` joined with every row of `right` [X3.135-1986 §5.20 GR2b].
    Product {
        /// The outer input.
        left: Box<Plan>,
        /// The inner input, read once.
        right: Box<Plan>,
    },
    /// The rows of `left` joined with the rows of `right` whose `keys` are
    /// equal: the rows of a product that its equalities keep
    /// [X3.135-1986 §5.20 GR2b, §5.21 GR2], in the same order (`physical/join.rs`).
    HashJoin {
        /// The outer input.
        left: Box<Plan>,
        /// The inner input, read once, into a hash table.
        right: Box<Plan>,
        /// Pairs of a column of `left` and a column of `right` that are equal.
        keys: Vec<(usize, usize)>,
        /// The equalities, as text.
        detail: String,
    },
    /// [ISO/IEC 9075:1992 §7.5] An outer join: each row of `left` with the
    /// rows of `right` whose `keys` are equal and for which `cond` is true,
    /// or, preserved, with nulls; then, if `right` is preserved, its rows
    /// that matched none, after nulls (`exec/join.rs`).
    OuterJoin {
        /// LEFT, RIGHT or FULL.
        kind: OuterJoin,
        /// The first operand, read as it is pulled.
        left: Box<Plan>,
        /// The second operand, read once.
        right: Box<Plan>,
        /// Pairs of a column of `left` and a column of `right` that are
        /// equal, found through a hash table.
        keys: Vec<(usize, usize)>,
        /// The rest of the `<join condition>`, over the pair's columns.
        cond: Option<Cond>,
        /// The degrees of `left` and `right`, the widths of the nulls.
        widths: (usize, usize),
        /// The condition, as text.
        detail: String,
    },
    /// Groups and their set functions [X3.135-1986 §5.22, §5.8].
    Aggregate {
        /// The grouping columns of `input`.
        groups: Vec<usize>,
        /// The set functions computed per group.
        aggs: Vec<AggCall>,
        /// The grouping and set functions, as text.
        detail: String,
        /// The rows grouped.
        input: Box<Plan>,
    },
    /// `row` if `input` has no rows, else the rows of `input` [X3.135-1986 §5.25 GR3].
    DefaultRow {
        /// The row given for no groups.
        row: Row,
        /// The grouped rows.
        input: Box<Plan>,
    },
    /// Each `<value expression>` applied to each row [X3.135-1986 §5.25 GR2].
    Project {
        /// The expressions, one per output column.
        exprs: Vec<Expr>,
        /// The expressions, as text.
        detail: String,
        /// The rows projected.
        input: Box<Plan>,
    },
    /// The rows of `input` with duplicates removed [X3.135-1986 §5.25 GR2].
    Distinct {
        /// The rows.
        input: Box<Plan>,
    },
    /// [ISO/IEC 9075:1992 §7.10 GR1b] The rows of `left` that are, or are
    /// not, in `right`: INTERSECT or EXCEPT \[ALL\] (`exec/set.rs`).
    SetOp {
        /// EXCEPT or INTERSECT.
        op: SetOp,
        /// ALL, which keeps duplicates.
        all: bool,
        /// The first operand, read as it is pulled.
        left: Box<Plan>,
        /// The second operand, read once.
        right: Box<Plan>,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] A system table's rows.
    System {
        /// The table, and whom its rows are made for, as it runs.
        read: SystemRead,
    },
    /// [ISO/IEC 9075:1992 §7.2] A row for each list of expressions.
    Values {
        /// The rows' expressions.
        rows: Vec<Vec<Expr>>,
        /// The rows, as text.
        detail: String,
    },
    /// The rows of the left input, then of the right (UNION ALL).
    Append {
        /// The rows first returned.
        left: Box<Plan>,
        /// The rows returned after them.
        right: Box<Plan>,
    },
    /// The rows of `input` in the order of `keys` [X3.135-1986 §8.3 GR3b].
    Sort {
        /// The sort keys, most significant first.
        keys: Vec<SortKey>,
        /// The keys, as text.
        detail: String,
        /// The rows sorted.
        input: Box<Plan>,
    },
}

/// \[DESIGN\] Chooses a physical plan for a logical one, using the state's
/// engines, indexes and cost model.
pub struct PhysicalPlanner<'a> {
    db: &'a State,
}

/// A built plan, and whether its rows come out in the order that was asked for.
struct Built {
    plan: Plan,
    ordered: bool,
}

impl Built {
    fn unordered(plan: Plan) -> Built {
        Built { plan, ordered: false }
    }
}

/// One table to read: its name, column names, and the columns the query uses.
#[derive(Clone, Copy)]
struct TableRead<'a> {
    table: &'a QualifiedName,
    names: &'a [String],
    needed: &'a [usize],
}

impl<'a> PhysicalPlanner<'a> {
    /// A planner over the state `db`.
    pub fn new(db: &'a State) -> Self {
        PhysicalPlanner { db }
    }

    /// The physical plan for `logical`.
    pub fn plan(&self, logical: &LogicalPlan) -> Plan {
        self.build(logical, &[]).plan
    }

    /// Build `logical`, preferring output ordered by the keys `want`, if
    /// any, where that pays, its products joined in the cheapest order
    /// (`physical/order.rs`).
    fn build(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        match self.join_order(logical, want) {
            Some(reordered) => self.build_node(&reordered, want),
            None => self.build_node(logical, want),
        }
    }

    /// Build `logical` as it is: a chain of products below it is not
    /// reordered (`build_node` of its operands), others are.
    fn build_node(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        match logical {
            LogicalPlan::Scan { table, names, needed } => self.access(TableRead { table, names, needed }, Vec::new(), want),
            LogicalPlan::Filter { cond, input } => self.filter(cond, input, want),

            // The nested loop yields rows in the order of its left input.
            LogicalPlan::Product { left, right } => {
                let want_left = of_left(want, left.width());
                let l = self.build_node(left, want_left);
                let r = self.build_node(right, &[]);
                Built {
                    plan: Plan::Product { left: Box::new(l.plan), right: Box::new(r.plan) },
                    ordered: !want_left.is_empty() && l.ordered,
                }
            }

            LogicalPlan::OuterJoin { kind, cond, left, right } => self.outer_join((*kind, cond), left, right, want),

            LogicalPlan::Aggregate { groups, aggs, input } => {
                let names = input.names();
                let detail = format!(
                    "group by [{}] computing [{}]",
                    groups.iter().map(|&g| column_name(&names, g)).collect::<Vec<_>>().join(", "),
                    aggs.iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", ")
                );
                // Groups come in the order of their first rows (`exec::aggregate`),
                // so rows in order of grouping columns give groups in that order.
                let inner: Vec<SortKey> = want
                    .iter()
                    .map(|k| groups.get(k.column).map(|&c| SortKey { column: c, descending: k.descending }))
                    .collect::<Option<_>>()
                    .unwrap_or_default();
                let b = self.build(input, &inner);
                let plan = Plan::Aggregate { groups: groups.clone(), aggs: aggs.clone(), detail, input: Box::new(b.plan) };
                Built { plan, ordered: !inner.is_empty() && b.ordered }
            }

            LogicalPlan::DefaultRow { row, input } => {
                let b = self.build(input, want);
                Built { plan: Plan::DefaultRow { row: row.clone(), input: Box::new(b.plan) }, ordered: b.ordered }
            }

            LogicalPlan::Project { exprs, input } => self.project(exprs, input, want),

            // Duplicate elimination keeps the first occurrence, so order is kept.
            LogicalPlan::Distinct { input } => {
                let b = self.build(input, want);
                Built { plan: Plan::Distinct { input: Box::new(b.plan) }, ordered: b.ordered }
            }

            LogicalPlan::SetOp { op, all, left, right } => self.set_operation((*op, *all), (left, right), want),
            LogicalPlan::System { read, .. } => Built::unordered(Plan::System { read: read.clone() }),
            LogicalPlan::Values { rows, .. } => {
                let row = |r: &Vec<Expr>| format!("({})", r.iter().map(|e| render_expr(e, &[])).collect::<Vec<_>>().join(", "));
                let detail = rows.iter().map(row).collect::<Vec<_>>().join(", ");
                Built::unordered(Plan::Values { rows: rows.clone(), detail })
            }

            LogicalPlan::Sort { keys, input } => self.sort(keys, input),
        }
    }

    /// [X3.135-1986 §8.3 GR2b] UNION is UNION ALL followed by duplicate
    /// elimination; EXCEPT and INTERSECT keep the first operand's rows in
    /// its order [ISO/IEC 9075:1992 §7.10 GR1b].
    fn set_operation(&self, (op, all): (SetOp, bool), (left, right): (&LogicalPlan, &LogicalPlan), want: &[SortKey]) -> Built {
        let r = self.build(right, &[]);
        if op == SetOp::Union {
            let l = self.build(left, &[]);
            let append = Plan::Append { left: Box::new(l.plan), right: Box::new(r.plan) };
            return Built::unordered(if all { append } else { Plan::Distinct { input: Box::new(append) } });
        }
        let l = self.build(left, want);
        Built { plan: Plan::SetOp { op, all, left: Box::new(l.plan), right: Box::new(r.plan) }, ordered: l.ordered }
    }

    /// A filter directly on a table becomes part of the access path; any
    /// other keeps the order of its input.
    fn filter(&self, cond: &Cond, input: &LogicalPlan, want: &[SortKey]) -> Built {
        if let LogicalPlan::Scan { table, names, needed } = input {
            let mut conds = Vec::new();
            split_and(cond.clone(), &mut conds);
            return self.access(TableRead { table, names, needed }, conds, want);
        }
        if let Some(join) = self.hash_join(cond, input, want) {
            return join;
        }
        let b = self.build_node(input, want);
        let detail = render_cond(cond, &input.names());
        Built { plan: Plan::Filter { cond: cond.clone(), detail, est: None, input: Box::new(b.plan) }, ordered: b.ordered }
    }

    /// A projected column that is a plain column reference keeps the input's
    /// order on that column.
    fn project(&self, exprs: &[Expr], input: &LogicalPlan, want: &[SortKey]) -> Built {
        let inner: Vec<SortKey> = want
            .iter()
            .map(|k| {
                let Some(Expr::Column(c)) = exprs.get(k.column) else { return None };
                Some(SortKey { column: *c, descending: k.descending })
            })
            .collect::<Option<_>>()
            .unwrap_or_default();
        let b = self.build(input, &inner);
        let detail = exprs.iter().map(|e| render_expr(e, &input.names())).collect::<Vec<_>>().join(", ");
        Built { plan: Plan::Project { exprs: exprs.to_vec(), detail, input: Box::new(b.plan) }, ordered: !inner.is_empty() && b.ordered }
    }

    /// The keys are asked of the input; if it gives them, no Sort is needed.
    fn sort(&self, keys: &[SortKey], input: &LogicalPlan) -> Built {
        let b = self.build(input, keys);
        if b.ordered {
            return Built::unordered(b.plan);
        }
        let detail = render_keys(keys, &input.names());
        Built::unordered(Plan::Sort { keys: keys.to_vec(), detail, input: Box::new(b.plan) })
    }
}

/// `want`, if each of its keys is of a product's left operand, `width`
/// columns wide, whose order the product keeps; else none.
fn of_left(want: &[SortKey], width: usize) -> &[SortKey] {
    if want.iter().all(|k| k.column < width) { want } else { &[] }
}

impl Plan {
    /// Whether running it can raise an error of a General Rule (an
    /// overflow, a division by zero, a subquery's cardinality): through a
    /// condition or expression that can, or a set function, which can
    /// overflow. Reading a table can fail too, but that is the storage's
    /// failure, not the statement's.
    pub fn can_raise(&self) -> bool {
        match self {
            Plan::SeqScan { .. } | Plan::IndexScan { .. } => false,
            // A system table may fail to make its rows.
            Plan::System { .. } => true,
            Plan::Filter { cond, input, .. } => cond.can_raise() || input.can_raise(),
            Plan::Project { exprs, input, .. } => exprs.iter().any(Expr::can_raise) || input.can_raise(),
            Plan::Aggregate { aggs, input, .. } => !aggs.is_empty() || input.can_raise(),
            Plan::Product { left, right }
            | Plan::HashJoin { left, right, .. }
            | Plan::Append { left, right }
            | Plan::SetOp { left, right, .. } => left.can_raise() || right.can_raise(),
            Plan::Values { rows, .. } => rows.iter().flatten().any(Expr::can_raise),
            Plan::OuterJoin { left, right, cond, .. } => {
                left.can_raise() || right.can_raise() || cond.as_ref().is_some_and(Cond::can_raise)
            }
            Plan::DefaultRow { input, .. } | Plan::Distinct { input } | Plan::Sort { input, .. } => input.can_raise(),
        }
    }

    /// The plan as indented text, one operator per line.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        // Writing to a String cannot fail.
        let _ = match self {
            Plan::SeqScan { table, rows, cost, order, .. } => {
                let ordered = if order.is_empty() { String::new() } else { format!(" in order of {}", order.len()) };
                writeln!(out, "{pad}SeqScan {table}{ordered}  (rows={rows}, cost={cost:.1})")
            }
            Plan::IndexScan { table, index, method, detail, est, cost, reverse, .. } => writeln!(
                out,
                "{pad}IndexScan {table} using {index} [{method}]{}: {detail}  (est. rows={est:.0}, cost={cost:.1})",
                if *reverse { " backward" } else { "" }
            ),
            Plan::Filter { detail, est: None, .. } => writeln!(out, "{pad}Filter {detail}"),
            Plan::Filter { detail, est: Some(est), .. } => writeln!(out, "{pad}Filter {detail}  (est. rows={est:.0})"),
            Plan::Product { .. } => writeln!(out, "{pad}NestedLoop (Cartesian product)"),
            Plan::HashJoin { detail, .. } => writeln!(out, "{pad}HashJoin {detail}"),
            Plan::OuterJoin { kind, detail, .. } => writeln!(out, "{pad}{kind:?}OuterJoin {detail}"),
            Plan::Aggregate { detail, .. } => writeln!(out, "{pad}Aggregate {detail}"),
            Plan::DefaultRow { .. } => writeln!(out, "{pad}DefaultRow (one row if no groups, [X3.135-1986 §5.25 GR3])"),
            Plan::Project { detail, .. } => writeln!(out, "{pad}Project {detail}"),
            Plan::Distinct { .. } => writeln!(out, "{pad}Distinct"),
            Plan::Append { .. } => writeln!(out, "{pad}Append"),
            Plan::SetOp { op, all, .. } => writeln!(out, "{pad}{op:?}{}", if *all { " all" } else { "" }),
            Plan::Values { detail, .. } => writeln!(out, "{pad}Values {detail}"),
            Plan::System { read } => writeln!(out, "{pad}System {}", read.name()),
            Plan::Sort { detail, .. } => writeln!(out, "{pad}Sort {detail}"),
        };
        match self {
            Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::Values { .. } | Plan::System { .. } => {}
            Plan::Product { left, right }
            | Plan::HashJoin { left, right, .. }
            | Plan::OuterJoin { left, right, .. }
            | Plan::Append { left, right }
            | Plan::SetOp { left, right, .. } => {
                left.write(out, depth.saturating_add(1));
                right.write(out, depth.saturating_add(1));
            }
            Plan::Filter { input, .. }
            | Plan::Aggregate { input, .. }
            | Plan::DefaultRow { input, .. }
            | Plan::Project { input, .. }
            | Plan::Distinct { input }
            | Plan::Sort { input, .. } => input.write(out, depth.saturating_add(1)),
        }
    }
}

// Unit tests: the planner's response to a state that breaks its invariants
// (RULES.md R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::catalog::{Catalog, MemoryCatalog, TableDesc};
    use crate::engine::rows::ROW_STORE;
    use crate::error::{Code, Result};
    use crate::exec::execute;
    use crate::txn::NO_TRANSACTION;

    /// Verifies: [DESIGN] a catalog table without storage is planned as a
    /// scan, which then reports SQLCODE -910 instead of the planner panicking.
    #[test]
    fn a_table_without_storage_is_scanned_and_reported() {
        let name = QualifiedName { auth: "A".into(), ident: "T".into() };
        let mut catalog = MemoryCatalog::default();
        catalog.create_schema(
            "A",
            "A",
            vec![TableDesc {
                name: name.clone(),
                columns: vec![],
                constraints: vec![],
                engine: ROW_STORE.into(),
                options: Vec::new(),
                generation: 0,
                temporary: None,
            }],
        );
        let st = Arc::new(State::with_builtins(Box::new(catalog)));
        let conds = vec![Cond::Const(crate::logical::Truth::True)];
        let built = PhysicalPlanner::new(&st).access(TableRead { table: &name, names: &[], needed: &[] }, conds, &[]);
        assert!(matches!(&built.plan, Plan::Filter { input, .. } if matches!(**input, Plan::SeqScan { .. })));
        let failed = execute(built.plan, &st, NO_TRANSACTION).collect::<Result<Vec<_>>>();
        assert_eq!(failed.err().map(|e| e.code), Some(Code::Api));
    }
}
