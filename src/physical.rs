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
mod collated;
mod cost;
mod estimate;
mod indexed;
mod join;
mod kept;
mod order;
mod probe;
mod shown;

pub use cost::{CostModel, DEFAULT_EQUAL, DEFAULT_INDEX_DESCENT, DEFAULT_NULL, DEFAULT_RANGE, DEFAULT_REORDER_GAIN, DEFAULT_SORT_COMPARE};
pub use kept::{Place, ProbeSlot};
pub(crate) use kept::{filled_plan, keepable, same_shape};

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::exec::WorkRows;
use crate::index::Probe;
use crate::logical::{
    AggCall, Cond, Expr, Fixpoint, LogicalPlan, OuterJoin, SetOp, SortKey, column_name, judged_by_sets, render_cond, render_expr,
    render_keys, split_and,
};
use crate::system::SystemRead;
use crate::value::Value;
use crate::value::collation::CollationRef;
use shown::Shown;

/// \[DESIGN\] A physical plan: the operators the executor runs, each with what
/// EXPLAIN shows of it. Cloned as a prepared statement's kept plan is
/// filled for an execution (`physical/kept.rs`).
#[derive(Debug, Clone)]
pub enum Plan {
    /// Every row of a table, in the engine's order, or in that of `order`.
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
        /// (`TableStorage::scan_ordered`), or backward; none, in no
        /// particular order.
        order: Vec<usize>,
        /// Read in that order backward: each column descending.
        backward: bool,
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
        /// The `?`s of a prepared statement among the probe's values, a
        /// null standing in for each until an execution fills it
        /// (`physical/kept.rs`); none of a plan made with values.
        slots: Vec<ProbeSlot>,
        /// Read the index backwards (for ORDER BY ... DESC).
        reverse: bool,
        /// The table's columns the query uses, the only ones a row found
        /// is read of (`TableStorage::get_columns`).
        columns: Vec<usize>,
        /// The condition the probe answers, as text.
        detail: String,
        /// The estimate of the rows found: the index's, or else from the
        /// table's statistics.
        est: f64,
        /// The planner's cost estimate.
        cost: f64,
        /// Whether every column the query reads of the table is of the
        /// index's key, and restored from it (`engine::key::restores`): a
        /// row its table says every reader sees alone is then made of its
        /// entry, not read (`TableStorage::visible_to_all`, RULES.md D34).
        covering: bool,
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
    /// The rows of `left` joined with the rows of a table an index finds
    /// for each, by the values of its `keys` (`physical/indexed.rs`): the
    /// rows of a product that its equalities keep [X3.135-1986 §5.20 GR2b,
    /// §5.21 GR2], or, `outer`, of a LEFT outer join [ISO/IEC 9075:1992
    /// §7.5], each row of `left` no table row matches with nulls; in the
    /// order of `left`, each row's matches in the index's (`exec/join/indexed.rs`).
    IndexJoin {
        /// A LEFT outer join.
        outer: bool,
        /// The outer input, each of its rows probing the index.
        left: Box<Plan>,
        /// The table read.
        table: QualifiedName,
        /// The index probed.
        index: String,
        /// The index's method, for EXPLAIN.
        method: String,
        /// The columns of `left` whose values each probe looks for, of the
        /// index's first columns in order.
        keys: Vec<usize>,
        /// The rest of the condition, over the pair's columns: the table's
        /// own filter's, and the join's.
        cond: Option<Cond>,
        /// The table's degree, the width of its nulls.
        width: usize,
        /// The table's columns the query uses, the only ones a row found
        /// is read of (`TableStorage::get_columns`).
        columns: Vec<usize>,
        /// The condition, as text.
        detail: String,
        /// The rows each probe is estimated to find.
        est: f64,
        /// The planner's cost estimate of all its probes.
        cost: f64,
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
        /// Its conjuncts that can raise an error over the columns of one
        /// operand alone, the left's and the right's, each over that
        /// operand's own columns: each evaluated on every row of its
        /// operand when the other has a row, as on every row of their
        /// product [ISO/IEC 9075:1992 §7.5 GR1c], and a row it is not true
        /// of matches none (RULES.md D15). Boxed, as few joins have them
        /// and every frame that holds a plan holds its largest variant.
        own: Own,
        /// The degrees of `left` and `right`, the widths of the nulls.
        widths: (usize, usize),
        /// The condition, as text.
        detail: String,
    },
    /// Groups and their set functions [X3.135-1986 §5.22, §5.8].
    Aggregate {
        /// The grouping columns of `input`.
        groups: Vec<usize>,
        /// What each groups by, if one is not the default collation.
        collations: Vec<Option<CollationRef>>,
        /// The set functions computed per group.
        aggs: Vec<AggCall>,
        /// The grouping and set functions, as text.
        detail: String,
        /// The rows grouped.
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
        /// How many leading columns rows are compared by, if not all.
        compared: Option<usize>,
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
    /// [ISO/IEC 9075-2:2023 §7.17 GR2c ii] The rows of member `member` of
    /// a recursive stratum: its members' plans run to their fixpoint
    /// (`exec/recursive.rs`).
    Recursive {
        /// Which member's rows, by its place among them.
        member: usize,
        /// The members' plans, and how their fixpoint is found.
        fixpoint: Fixpoint<Plan>,
    },
    /// The rows a member of the recursive stratum around it had after the
    /// last iteration: none until the `Recursive` it is read in gives them
    /// to it.
    WorkTable {
        /// Which member, by its place among them.
        member: usize,
        /// Its rows, once given.
        rows: Option<WorkRows>,
    },
}

/// \[DESIGN\] Chooses a physical plan for a logical one, using the state's
/// engines, indexes and cost model.
#[derive(Clone, Copy)]
pub struct PhysicalPlanner<'a> {
    db: &'a State,
    /// Whether the plan is made to be shown, its operators described
    /// (`physical/shown.rs`), as EXPLAIN's is; one made to run is not.
    shown: bool,
    /// Of a plan made with a prepared statement's slots, the values of the
    /// execution its paths are chosen by (`physical/kept.rs`).
    values: Option<&'a [Value]>,
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

/// One table to read: its name, how the operators over it are described
/// (`physical/shown.rs`), and the columns the query uses.
#[derive(Clone, Copy)]
struct TableRead<'a> {
    table: &'a QualifiedName,
    shown: Shown<'a>,
    needed: &'a [usize],
}

impl<'a> PhysicalPlanner<'a> {
    /// A planner over the state `db`, of plans to run.
    pub fn new(db: &'a State) -> Self {
        PhysicalPlanner { db, shown: false, values: None }
    }

    /// A planner over the state `db`, of plans to run made with a prepared
    /// statement's slots, their paths chosen by `values`, one execution's,
    /// so that another's may fill them (`physical/kept.rs`).
    pub fn slotted(db: &'a State, values: &'a [Value]) -> Self {
        PhysicalPlanner { db, shown: false, values: Some(values) }
    }

    /// A planner over the state `db`, of plans to show: each operator
    /// described, as EXPLAIN shows it.
    pub fn describing(db: &'a State) -> Self {
        PhysicalPlanner { db, shown: true, values: None }
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
            LogicalPlan::Scan { table, names, needed } => {
                self.access(TableRead { table, shown: self.showing(names), needed }, Vec::new(), want)
            }
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

            LogicalPlan::Aggregate { .. } => self.aggregate(logical, want),

            LogicalPlan::Project { exprs, input } => self.project(exprs, input, want),

            // Duplicate elimination keeps the first occurrence, so order is kept.
            LogicalPlan::Distinct { input } => {
                let b = self.build(input, want);
                Built { plan: Plan::Distinct { input: Box::new(b.plan) }, ordered: b.ordered }
            }

            LogicalPlan::SetOp { .. } => self.set_operation(logical, want),
            LogicalPlan::System { read, .. } => Built::unordered(Plan::System { read: read.clone() }),
            LogicalPlan::Values { rows, .. } => {
                let row = |r: &Vec<Expr>| format!("({})", r.iter().map(|e| render_expr(e, &[])).collect::<Vec<_>>().join(", "));
                let detail = if self.shown { rows.iter().map(row).collect::<Vec<_>>().join(", ") } else { String::new() };
                Built::unordered(Plan::Values { rows: rows.clone(), detail })
            }

            LogicalPlan::Sort { keys, input } => self.sort(keys, input),
            LogicalPlan::Recursive { member, fixpoint, .. } => {
                Built::unordered(Plan::Recursive { member: *member, fixpoint: fixpoint.map_ref(|p| self.plan(p)) })
            }
            LogicalPlan::WorkTable { member, .. } => Built::unordered(Plan::WorkTable { member: *member, rows: None }),
        }
    }

    /// The groups of `logical`, an aggregation, in the order of `want` if
    /// their first rows' order can give it.
    fn aggregate(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        let LogicalPlan::Aggregate { groups, collations, aggs, input } = logical else { return self.build_node(logical, want) };
        let detail = self.described(input, |names| {
            format!(
                "group by [{}] computing [{}]",
                groups.iter().map(|&g| column_name(names, g)).collect::<Vec<_>>().join(", "),
                aggs.iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", ")
            )
        });
        // Groups come in the order of their first rows (`exec/group.rs`),
        // so rows in order of grouping columns give groups in that order.
        let inner: Vec<SortKey> = want
            .iter()
            .map(|k| groups.get(k.column).filter(|_| collations.is_empty()).map(|&c| SortKey { column: c, descending: k.descending }))
            .collect::<Option<_>>()
            .unwrap_or_default();
        let b = self.build(input, &inner);
        let plan =
            Plan::Aggregate { groups: groups.clone(), collations: collations.clone(), aggs: aggs.clone(), detail, input: Box::new(b.plan) };
        Built { plan, ordered: !inner.is_empty() && b.ordered }
    }

    /// [X3.135-1986 §8.3 GR2b] UNION is UNION ALL followed by duplicate
    /// elimination; EXCEPT and INTERSECT keep the first operand's rows in
    /// its order [ISO/IEC 9075:1992 §7.10 GR1b].
    fn set_operation(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        let LogicalPlan::SetOp { op, all, compared, left, right } = logical else { return self.build_node(logical, want) };
        let (op, all, compared) = (*op, *all, *compared);
        let r = self.build(right, &[]);
        if op == SetOp::Union {
            let l = self.build(left, &[]);
            let append = Plan::Append { left: Box::new(l.plan), right: Box::new(r.plan) };
            return Built::unordered(if all { append } else { Plan::Distinct { input: Box::new(append) } });
        }
        let l = self.build(left, want);
        Built { plan: Plan::SetOp { op, all, compared, left: Box::new(l.plan), right: Box::new(r.plan) }, ordered: l.ordered }
    }

    /// A filter directly on a table becomes part of the access path; any
    /// other keeps the order of its input, its long IN lists judged by
    /// sets (`logical::judged_by_sets`).
    fn filter(&self, cond: &Cond, input: &LogicalPlan, want: &[SortKey]) -> Built {
        if let LogicalPlan::Scan { table, names, needed } = input {
            let mut conds = Vec::new();
            split_and(cond.clone(), &mut conds);
            return self.access(TableRead { table, shown: self.showing(names), needed }, conds, want);
        }
        if let Some(join) = self.hash_join(cond, input, want) {
            return join;
        }
        let b = self.build_node(input, want);
        let cond = judged_by_sets(cond.clone());
        let detail = self.described(input, |names| render_cond(&cond, names));
        Built { plan: Plan::Filter { cond, detail, est: None, input: Box::new(b.plan) }, ordered: b.ordered }
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
        let detail = self.described(input, |names| exprs.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", "));
        Built { plan: Plan::Project { exprs: exprs.to_vec(), detail, input: Box::new(b.plan) }, ordered: !inner.is_empty() && b.ordered }
    }

    /// The keys are asked of the input; if it gives them, no Sort is needed.
    fn sort(&self, keys: &[SortKey], input: &LogicalPlan) -> Built {
        let b = self.build(input, keys);
        if b.ordered {
            return Built::unordered(b.plan);
        }
        let detail = self.described(input, |names| render_keys(keys, names));
        Built::unordered(Plan::Sort { keys: keys.to_vec(), detail, input: Box::new(b.plan) })
    }
}

/// `want`, if each of its keys is of a product's left operand, `width`
/// columns wide, whose order the product keeps; else none.
fn of_left(want: &[SortKey], width: usize) -> &[SortKey] {
    if want.iter().all(|k| k.column < width) { want } else { &[] }
}

/// An outer join's own conditions, of its left operand's columns and of
/// its right's (`Plan::OuterJoin`), if either has one.
pub type Own = Option<Box<(Option<Cond>, Option<Cond>)>>;

impl Plan {
    /// Whether running it can raise an error of a General Rule (an
    /// overflow, a division by zero, a subquery's cardinality): through a
    /// condition or expression that can, or a set function, which can
    /// overflow. Reading a table can fail too, but that is the storage's
    /// failure, not the statement's.
    pub fn can_raise(&self) -> bool {
        match self {
            Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::WorkTable { .. } => false,
            // Past EXECUTOR.MAX_ITERATIONS a recursive query is refused,
            // and a system table may fail to make its rows.
            Plan::Recursive { .. } | Plan::System { .. } => true,
            Plan::Filter { cond, input, .. } => cond.can_raise() || input.can_raise(),
            Plan::Project { exprs, input, .. } => exprs.iter().any(Expr::can_raise) || input.can_raise(),
            Plan::Aggregate { aggs, input, .. } => !aggs.is_empty() || input.can_raise(),
            Plan::Product { left, right }
            | Plan::HashJoin { left, right, .. }
            | Plan::Append { left, right }
            | Plan::SetOp { left, right, .. } => left.can_raise() || right.can_raise(),
            Plan::Values { rows, .. } => rows.iter().flatten().any(Expr::can_raise),
            Plan::OuterJoin { left, right, cond, own, .. } => {
                let owned = own.as_deref().is_some_and(|(l, r)| [l, r].iter().any(|c| c.as_ref().is_some_and(Cond::can_raise)));
                left.can_raise() || right.can_raise() || owned || cond.as_ref().is_some_and(Cond::can_raise)
            }
            Plan::IndexJoin { left, cond, .. } => left.can_raise() || cond.as_ref().is_some_and(Cond::can_raise),
            Plan::Distinct { input } | Plan::Sort { input, .. } => input.can_raise(),
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
        let built = PhysicalPlanner::new(&st).access(TableRead { table: &name, shown: Shown::of(None), needed: &[] }, conds, &[]);
        assert!(matches!(&built.plan, Plan::Filter { input, .. } if matches!(**input, Plan::SeqScan { .. })));
        let failed = execute(built.plan, &st, NO_TRANSACTION).collect::<Result<Vec<_>>>();
        assert_eq!(failed.err().map(|e| e.code), Some(Code::Api));
    }
}
