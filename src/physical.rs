//! \[DESIGN\] Physical planning: decide *how* to compute a logical plan.
//!
//! Nothing about storage is assumed here. Every number comes from a plugin:
//!   * the storage engine prices scans and row fetches (`TableStorage`),
//!   * each index estimates how many rows a probe returns (`Index`),
//!   * the storage engine's statistics estimate how many rows a condition
//!     keeps (`Statistics`, `physical/estimate.rs`),
//!   * each index method declares which comparisons and function calls it
//!     can use to find rows, and whether it gives key order (`IndexMethod`),
//!   * the remaining weights live in a `CostModel` the user can change.
//!
//! "Interesting orders" (System R: Selinger et al., "Access Path Selection in a Relational Database
//! Management System", SIGMOD 1979): when a Sort is required, the order is passed down through
//! operators that preserve it. A table access may then choose an ordered index path, and the Sort
//! is dropped if that is cheaper than scanning in any order and sorting.

mod access;
mod analyzed;
pub use analyzed::{Analyzed, Counted, NodeCount, Operator};
pub(crate) use analyzed::{counted, millis, operators};
mod built;
mod cardinality;
mod carried;
mod collated;
mod cost;
mod decided;
mod enumerate;
pub use decided::Decided;
pub(crate) mod estimate;
mod expression_index;
mod grouping;
mod indexed;
mod join_probe;
pub use join_probe::{Fields, JoinComparison, JoinProbe, Paired, Preserved, Spatial, Supplied, Unmatched};
mod join;
pub use join::JoinKey;
mod kept;
mod merged;
pub use merged::MergeJoin;
mod nearest;
mod nodes;
pub(crate) use nearest::distance_call;
pub use nearest::{ApproximateRead, NearestRead};
mod order;
mod ordered;
mod parallel;
pub use parallel::Parallel;
pub(crate) use parallel::{parallel, rows_of as parallel_rows, workers_of};
mod probe;
mod project;
mod range;
pub use range::{Bound, Pruned, RangeJoin};
mod raising;
mod recognized;
mod semi;
pub use semi::{SemiJoin, SemiMethod};
mod shown;
pub(crate) mod spatial;

pub use cardinality::Injected;
pub use cost::{CostModel, DEFAULT_EQUAL, DEFAULT_INDEX_DESCENT, DEFAULT_NULL, DEFAULT_RANGE, DEFAULT_REORDER_GAIN, DEFAULT_SORT_COMPARE};
pub use enumerate::{ADAPTIVE, DEFAULT_DP_BUDGET, DEFAULT_LINEARIZED, ENUMERATIONS, GREEDY};
pub use kept::{Place, ProbeSlot};
pub(crate) use kept::{filled_plan, keepable, same_shape};

use std::sync::Arc;

use built::Built;
use cardinality::Estimates;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::exec::{TransitionRows, WorkRows};
use crate::index::Probe;
use crate::logical::{
    AggCall, Cond, Expr, Fixpoint, Limit, LogicalPlan, Most, OuterJoin, Polymorphic, Recognize, SetOp, SortKey, Unnest, Window,
    judged_by_sets, render_cond,
};
use crate::storage::Row;
use crate::system::SystemRead;
use crate::value::Value;
use crate::value::collation::CollationRef;
use shown::Shown;

/// \[DESIGN\] A physical plan: the operators the executor runs, each with what
/// EXPLAIN shows of it. Cloned as a prepared statement's kept plan is
/// filled for an execution (`physical/kept.rs`).
#[derive(Debug, Clone)]
pub enum Plan {
    /// Distance candidates refined before the final SQL ordering.
    Nearest(Box<NearestRead>),
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
        /// Of its first rows in that order alone asked for, how many
        /// (`exec/limit.rs`).
        most: Option<usize>,
        /// Whether each row it reads is followed by the row's id
        /// (`LogicalPlan::Scan`).
        located: bool,
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
        /// Whether each row it reads is followed by the row's id
        /// (`LogicalPlan::Scan`).
        located: bool,
        /// \[DESIGN\] (RULES.md D85) A conjunct the scan decides of each
        /// entry, not its filter: of an entry that is one point, not the
        /// origin an empty geometry projects to, by the call's function
        /// from the point, its row read only if `columns` names one, and
        /// if every reader sees it; of any other, by the call on its row.
        decided: Option<Box<Decided>>,
    },
    /// The rows of `input` for which `cond` is true [X3.135-1986 §5.21 GR2].
    Filter {
        /// The condition.
        cond: Cond,
        /// The condition, as text.
        detail: String,
        /// The rows it is estimated to keep.
        est: f64,
        /// Whether it filters a table's read, its estimate the access
        /// path's, which EXPLAIN shows.
        of_read: bool,
        /// The rows filtered.
        input: Box<Plan>,
    },
    /// Every row of `left` joined with every row of `right` [X3.135-1986 §5.20 GR2b].
    Product {
        /// The outer input.
        left: Box<Plan>,
        /// The inner input, read once.
        right: Box<Plan>,
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
    },
    /// The rows of `left` joined with the rows of `right` whose `keys` are
    /// equal: the rows of a product that its equalities keep
    /// [X3.135-1986 §5.20 GR2b, §5.21 GR2], in the same order (`physical/join.rs`).
    HashJoin {
        /// The outer input.
        left: Box<Plan>,
        /// The inner input, read once, into a hash table.
        right: Box<Plan>,
        /// Pairs of a column of `left` and a column of `right` that are
        /// equal, or not distinct.
        keys: Vec<JoinKey>,
        /// The equalities, as text.
        detail: String,
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
    },
    /// The rows of `left` joined with the rows of a table an index finds
    /// for each, by its bound `probe` (`physical/indexed.rs`): the
    /// rows of a product that its comparisons keep [X3.135-1986 §5.20 GR2b,
    /// §5.21 GR2], or, `outer`, of a LEFT outer join [ISO/IEC 9075:1992
    /// §7.5], each row of `left` no table row matches with nulls; in the
    /// order of `left`, each row's matches in the index's (`exec/join/indexed.rs`),
    /// unless `spatial` permits an unordered scan alternative.
    IndexJoin {
        /// The rows matching none it keeps, of a LEFT or FULL outer join.
        outer: Preserved,
        /// None, probes alone; or, of an unordered fixed-SRID join, a scan
        /// of the table matched with the outer rows held, or a traversal
        /// of both tables' R-trees. Both geometry columns enforce one SRID
        /// and every residual is non-raising, so it needs no eager check.
        spatial: Option<Box<Spatial>>,
        /// The outer input, each of its rows probing the index.
        left: Box<Plan>,
        /// The table read.
        table: QualifiedName,
        /// The index probed.
        index: Box<str>,
        /// The index's method, for EXPLAIN.
        method: Box<str>,
        /// How each outer row supplies the index search bounds.
        probe: JoinProbe,
        /// The rest of the condition, over the pair's columns: the table's
        /// own filter's, and the join's.
        cond: Option<Cond>,
        /// The table's degree, the width of its nulls, and one more if
        /// `located`.
        width: usize,
        /// The table's columns the query uses, the only ones a row found
        /// is read of (`TableStorage::get_columns`).
        columns: Box<[usize]>,
        /// The condition, as text.
        detail: Box<str>,
        /// The rows each probe is estimated to find.
        est: f64,
        /// The planner's cost estimate of all its probes.
        cost: f64,
        /// Whether each table row found is followed by its id, of a LEFT
        /// join a null with the nulls (`LogicalPlan::Scan`).
        located: bool,
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
        /// equal, or not distinct, found through a hash table.
        keys: Vec<JoinKey>,
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
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
    },
    /// \[DESIGN\] A merge join (`physical/merged.rs`, RULES.md D303).
    MergeJoin(Box<MergeJoin>),
    /// \[DESIGN\] A range join (`physical/range.rs`, RULES.md D580).
    RangeJoin(Box<RangeJoin>),
    /// \[DESIGN\] A semi join or an antijoin (`physical/semi.rs`, RULES.md D540).
    SemiJoin(Box<SemiJoin>),
    /// Groups and their set functions [X3.135-1986 §5.22, §5.8].
    Aggregate {
        /// The grouping columns of `input`.
        groups: Vec<usize>,
        /// What each groups by, if one is not the default collation.
        collations: Vec<Option<CollationRef>>,
        /// The set functions computed per group.
        aggs: Vec<AggCall>,
        /// The values of groups made before any row
        /// (`LogicalPlan::Aggregate`).
        seeds: Vec<Row>,
        /// The grouping and set functions, as text.
        detail: String,
        /// The rows grouped.
        input: Box<Plan>,
        /// Its estimated groups (`physical/cardinality.rs`).
        est: f64,
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
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
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
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
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
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
    },
    /// \[EXTENSION\] The rows of each of `inputs`, each in the order of
    /// `keys`, merged in that order: the partitions of a partitioned table
    /// read (RULES.md D156).
    MergeAppend {
        /// The rows merged, each input in the order of `keys`.
        inputs: Vec<Plan>,
        /// The keys, most significant first.
        keys: Vec<SortKey>,
        /// What EXPLAIN shows of it: MergeAppend and the keys.
        detail: String,
    },
    /// The rows of `input` in the order of `keys` [X3.135-1986 §8.3 GR3b].
    Sort {
        /// The sort keys, most significant first.
        keys: Vec<SortKey>,
        /// The keys, as text.
        detail: String,
        /// The rows sorted.
        input: Box<Plan>,
        /// Of its first rows alone asked for, how many (`exec/limit.rs`).
        most: Option<Most>,
    },
    /// [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] The rows of `input` `limit`
    /// keeps, in its order (`exec/limit.rs`).
    Limit {
        /// Its counts.
        limit: Limit,
        /// The rows limited.
        input: Box<Plan>,
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
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
    /// [ISO/IEC 9075-2:2023 §4.46.2] The rows of a transition table, as
    /// given to the triggered action that reads it (`exec/transition.rs`).
    Transition {
        /// Its rows.
        rows: TransitionRows,
    },
    /// [ISO/IEC 9075-2:2023 §7.6 SR8] A `<collection derived table>`
    /// laterally over the rows of its input (`exec/unnest.rs`).
    Unnest(Box<Unnest<Plan>>),
    /// [ISO/IEC 9075-2:2023 §7.15] Window functions over the rows of its
    /// input, which come sorted by its partitioning and ordering
    /// (`exec/window.rs`).
    Window {
        /// The functions, and the rows they are computed over.
        window: Box<Window<Plan>>,
        /// The partitioning, ordering and functions, as text.
        detail: String,
        /// Its estimated rows (`physical/cardinality.rs`).
        est: f64,
    },
    /// [ISO/IEC 9075-2:2023 §7.7] Row pattern recognition over the rows of
    /// its input, which come sorted by its partitioning and ordering
    /// (`exec/pattern.rs`).
    Recognize {
        /// The recognition, and the rows it is sought in.
        recognize: Box<Recognize<Plan>>,
        /// The partitioning, ordering, measures and pattern, as text.
        detail: String,
    },
    /// [ISO/IEC 9075-2:2023 §7.6, §9.25] A `<PTF derived table>`: its
    /// component procedures run on its table arguments' rows, each read
    /// whole first (`exec/ptf.rs`).
    Polymorphic(Box<Polymorphic<Plan>>),
    /// \[DESIGN\] Its input run by morsels on threads at once, its rows in order (RULES.md D290).
    Parallel(Box<Parallel>),
    /// \[EXTENSION\] An operator EXPLAIN ANALYZE counts, run as its input (RULES.md D376).
    Counted(Box<Counted>),
}

/// \[DESIGN\] Chooses a physical plan for a logical one, using the state's
/// engines, indexes and cost model.
#[derive(Clone, Copy)]
pub struct PhysicalPlanner<'a> {
    db: &'a State,
    /// Whether the plan is made to be shown, its operators described (`physical/shown.rs`), as EXPLAIN's is; one made to run is not.
    shown: bool,
    /// Of a plan made with a prepared statement's slots, the values of the
    /// execution its paths are chosen by (`physical/kept.rs`).
    values: Option<&'a [Value]>,
    /// Of the input of a FETCH FIRST, the most of its first rows read, if
    /// known as planned (`Limit::read_rows`): through a sort and a
    /// projection to a scan, whose path in the order asked is priced by
    /// them (`physical/access.rs`). Below any other node, none.
    first: Option<usize>,
    /// \[EXTENSION\] The FETCH FIRST whose count `first` is: its search.
    fetch: Option<&'a crate::logical::Fetch>,
    /// The planning's estimator (`physical/cardinality.rs`), made by `plan`.
    est: Option<&'a Estimates<'a>>,
}

/// One table to read: its name, how the operators over it are described
/// (`physical/shown.rs`), the columns the query uses, and whether each row
/// read is followed by its id (`LogicalPlan::Scan`).
#[derive(Clone, Copy)]
struct TableRead<'a> {
    table: &'a QualifiedName,
    shown: Shown<'a>,
    needed: &'a [usize],
    located: bool,
    /// The columns the node above a filter over the table reads of its
    /// rows, if known: a projection's or a grouping's (`build_input`);
    /// none of any other. A scan deciding a conjunct itself reads the
    /// row of an entry it decides only for these (`Plan::IndexScan`).
    above: Option<&'a [usize]>,
}

impl<'a> PhysicalPlanner<'a> {
    /// A planner over the state `db`, of plans to run.
    pub fn new(db: &'a State) -> Self {
        PhysicalPlanner { db, shown: false, values: None, first: None, fetch: None, est: None }
    }

    /// A planner over the state `db`, of plans to run made with a prepared
    /// statement's slots, their paths chosen by `values`, one execution's,
    /// so that another's may fill them (`physical/kept.rs`).
    pub fn slotted(db: &'a State, values: &'a [Value]) -> Self {
        PhysicalPlanner { db, shown: false, values: Some(values), first: None, fetch: None, est: None }
    }

    /// A planner over the state `db`, of plans to show: each operator
    /// described, as EXPLAIN shows it.
    pub fn describing(db: &'a State) -> Self {
        PhysicalPlanner { db, shown: true, values: None, first: None, fetch: None, est: None }
    }

    /// The physical plan for `logical`, estimated by one estimator for
    /// the whole planning (RULES.md D370).
    pub fn plan(&self, logical: &LogicalPlan) -> Plan {
        if self.est.is_some() {
            return self.build(logical, &[]).plan;
        }
        let estimates = Estimates::over(self.db, logical);
        PhysicalPlanner { est: Some(&estimates), ..*self }.build(logical, &[]).plan
    }

    /// Build `logical`, preferring output ordered by the keys `want`, if
    /// any, where that pays, its joins enumerated (`physical/enumerate.rs`)
    /// or ordered greedily (`physical/order.rs`).
    fn build(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        self.ordered_joins(logical, want)
    }

    /// Build `logical` as it is: a chain of products below it is not
    /// reordered (`build_node` of its operands), others are. The first
    /// rows a FETCH FIRST reads are known through a scan, a projection or
    /// a sort alone: a filter, a join or a grouping reads more of its input
    /// than it gives.
    fn build_node(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        if self.first.is_some() && !matches!(logical, LogicalPlan::Scan { .. } | LogicalPlan::Project { .. } | LogicalPlan::Sort { .. }) {
            return PhysicalPlanner { first: None, ..*self }.build_node(logical, want);
        }
        match logical {
            LogicalPlan::Scan { table, names, needed, located } => {
                let read = TableRead { table, shown: self.showing(names), needed, located: *located, above: None };
                self.access(read, Vec::new(), want)
            }
            LogicalPlan::Filter { cond, input } => self.filter(cond, input, want, None),
            LogicalPlan::Product { left, right } => self.product(left, right, want),
            LogicalPlan::OuterJoin { kind, cond, left, right } => self.outer_join((*kind, cond), left, right, want),
            LogicalPlan::SemiJoin { anti, cond, left, right } => self.semi_join((*anti, cond), (left, right), want),
            LogicalPlan::Aggregate { .. } => self.aggregate(logical, want),
            LogicalPlan::Project { exprs, input } => self.project(exprs, input, want),
            LogicalPlan::Distinct { input } => self.distinct(input, want),
            LogicalPlan::SetOp { .. } => self.set_operation(logical, want),
            LogicalPlan::System { read, .. } => Self::system(read),
            LogicalPlan::Values { rows, .. } => self.values(rows),
            LogicalPlan::Sort { keys, input } => self.sort(keys, input),
            LogicalPlan::Limit { limit, input } => self.limited(limit, input, want),
            LogicalPlan::Recursive { member, fixpoint, .. } => self.recursive(*member, fixpoint),
            LogicalPlan::WorkTable { member, .. } => Self::work_table(*member),
            LogicalPlan::Transition { rows, .. } => Built::unordered(Plan::Transition { rows: rows.clone() }),
            LogicalPlan::Unnest(u) => self.unnest(u),
            LogicalPlan::Window(w) => self.window(w, want),
            LogicalPlan::Recognize(r) => self.recognize(r),
            // Each input is read whole, in no order of its own: its rows are
            // partitioned and ordered as the function says.
            LogicalPlan::Polymorphic(p) => Built::unordered(Plan::Polymorphic(Box::new(Polymorphic {
                call: Arc::clone(&p.call),
                arguments: p.arguments.clone(),
                inputs: p.inputs.iter().map(|input| self.build(input, &[]).plan).collect(),
                names: p.names.clone(),
            }))),
        }
    }

    /// A filter directly on a table, or over a table's read and a guard
    /// (`physical/access/guarded.rs`), becomes part of the access path,
    /// told the columns the node above reads, `above`, if known; any other
    /// keeps the order of its input, its long IN lists judged by sets
    /// (`logical::judged_by_sets`).
    fn filter(&self, cond: &Cond, input: &LogicalPlan, want: &[SortKey], above: Option<&[usize]>) -> Built {
        if let Some(read) = self.filtered_read(cond, input, (want, above)) {
            return read;
        }
        if let Some(join) = self.range_join(cond, input, want) {
            return join;
        }
        if let Some(join) = self.hash_join(cond, input, want) {
            return join;
        }
        let b = self.build_node(input, want);
        let est = self.estimating(|e| e.filter_card(cond, input)).rows;
        let cond = judged_by_sets(cond.clone());
        let detail = self.described(input, |names| render_cond(&cond, names));
        Built { plan: Plan::Filter { cond, detail, est, of_read: false, input: Box::new(b.plan) }, ordered: b.ordered }
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

// Unit tests: the planner's response to a state that breaks its invariants
// (RULES.md R-TEST-2 robustness), and the orders its plans give.
#[cfg(test)]
mod tests;
