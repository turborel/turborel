//! \[DESIGN\] The logical plan: relational algebra over bound expressions.
//!
//! It says *what* to compute and knows nothing about storage, so rewrite
//! rules (optimizer.rs) can work on it without knowing which engine, index or
//! layout will be used. The physical planner (physical.rs) decides *how*.
//!
//! Columns are positions. Each node's output row is described by `names()`:
//! a Scan outputs its table's columns, a Product the left's then the right's,
//! an Aggregate its grouping columns then one column per set function.

mod dynamic;
mod expr;
mod fixpoint;
mod pushed;
mod render;
mod sets;
mod subquery;
mod window;

pub use dynamic::{filled, filled_cond, filled_expr};
pub use expr::{
    Cond, DomainChecks, Expr, Quantifier, Slot, Truth, and_all, cond_columns, expr_columns, map_cond_columns, map_cond_exprs,
    map_expr_columns, or_all, split_and,
};
pub use fixpoint::Fixpoint;
pub use pushed::{compared_with_constant, disjuncts, in_list, like_bounds, pushed_conjuncts};
pub use render::{column_name, render_cond, render_expr, render_keys};
pub use sets::{IN_SET_LEAST, InSet, judged_by_sets};
pub use subquery::{Decorrelated, Subquery, map_plan_exprs, plan_has_outer};
pub use window::{Bound, Exclusion, Frame, Marker, Nested, Offset, Pick, Units, Valued, Window, WindowCall, WindowKind};

use crate::catalog::QualifiedName;
use crate::functions::AggregateRef;
use crate::storage::Row;
use crate::system::SystemRead;
use crate::value::collation::CollationRef;
use crate::value::{DataType, SortOrder};

/// One set function to compute per group.
#[derive(Debug, Clone)]
pub struct AggCall {
    /// The set function.
    pub func: AggregateRef,
    /// None for COUNT(*); otherwise evaluated against each row of the group.
    pub arg: Option<Expr>,
    /// Whether duplicate values are removed first [X3.135-1986 §5.8 GR1].
    pub distinct: bool,
    /// [ISO/IEC 9075-2:2023 §10.9 GR4] The condition of its `<filter
    /// clause>`, if written: a row it is not true of is not fed to it.
    pub filter: Option<Cond>,
    /// [ISO/IEC 9075-2:2023 §10.9 GR10] Of an inverse distribution
    /// function, what it picks of its values, which its grouping sorts.
    pub distribution: Option<Box<Distribution>>,
    /// The type of the result.
    pub result: DataType,
    /// The call as written, e.g. "COUNT(DISTINCT E.DEPT)": its column name and
    /// how repeats of one call are found.
    pub label: String,
}

/// [ISO/IEC 9075-2:2023 §10.9 GR10] What an inverse distribution function
/// picks of its values, its argument sorted.
#[derive(Debug, Clone)]
pub struct Distribution {
    /// PERCENTILE_CONT, which interpolates, rather than PERCENTILE_DISC.
    pub continuous: bool,
    /// The direction of its `<sort specification>`; a null value is
    /// eliminated, so its null ordering places none.
    pub order: SortOrder,
    /// NVE, its `<inverse distribution function argument>`, over a row of
    /// the group: not aggregated \[SR14\], so alike of every row of it.
    pub fraction: Expr,
}

impl AggCall {
    /// Its expressions: its argument, those of its filter, and its
    /// fraction.
    pub fn exprs(&self) -> Vec<&Expr> {
        let mut out: Vec<&Expr> = self.arg.iter().collect();
        if let Some(c) = &self.filter {
            c.for_each_expr(&mut |e| out.push(e));
        }
        out.extend(self.distribution.iter().map(|d| &d.fraction));
        out
    }

    /// Its expressions, as `exprs` gives them, each replaced by `f` of it.
    #[must_use]
    pub fn map_exprs(self, f: &mut dyn FnMut(Expr) -> Expr) -> AggCall {
        let distribution = self.distribution.map(|d| Box::new(Distribution { fraction: f(d.fraction.clone()), ..*d }));
        AggCall { arg: self.arg.map(&mut *f), filter: self.filter.map(|c| c.map_exprs(f)), distribution, ..self }
    }
}

/// [X3.135-1986 §8.3 GR3b] a sort column of the node's output, and its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    /// The column's position in the node's output.
    pub column: usize,
    /// DESC rather than ASC.
    pub descending: bool,
    /// [ISO/IEC 9075-2:2023 §10.10 GR1e ii] NULLS FIRST rather than NULLS
    /// LAST, as written or implied (`SortOrder::of`).
    pub nulls_first: bool,
}

impl SortKey {
    /// Column `column` in the order `order`.
    pub fn new(column: usize, order: SortOrder) -> SortKey {
        SortKey { column, descending: order.descending, nulls_first: order.nulls_first }
    }

    /// Column `column`, ascending, nulls placed as implied.
    pub fn ascending(column: usize) -> SortKey {
        SortKey::new(column, SortOrder::of(false, None))
    }

    /// How it orders its column's values.
    pub fn order(&self) -> SortOrder {
        SortOrder { descending: self.descending, nulls_first: self.nulls_first }
    }
}

/// \[DESIGN\] A logical plan node. Each node's inputs are logical plans too.
#[derive(Debug, Clone)]
pub enum LogicalPlan {
    /// A base table named in a `<from clause>`.
    Scan {
        /// The table.
        table: QualifiedName,
        /// Its columns' display names.
        names: Vec<String>,
        /// The columns the query uses, which a storage engine may use to price a scan.
        needed: Vec<usize>,
    },
    /// [X3.135-1986 §5.21, §5.23] keep rows for which the condition is true.
    Filter {
        /// The condition.
        cond: Cond,
        /// The rows filtered.
        input: Box<LogicalPlan>,
    },
    /// [X3.135-1986 §5.20 GR2b] the extended Cartesian product.
    Product {
        /// The first operand: its columns come first.
        left: Box<LogicalPlan>,
        /// The second operand.
        right: Box<LogicalPlan>,
    },
    /// [X3.135-1986 §5.22, §5.8] groups and set functions.
    Aggregate {
        /// The grouping columns of `input`.
        groups: Vec<usize>,
        /// \[EXTENSION\] What each groups by, if one is not the default
        /// collation: rows are of a group by their sort keys, and the group
        /// shows its first row's values [ISO/IEC 9075:1992 §8.2 GR3d].
        collations: Vec<Option<CollationRef>>,
        /// The set functions computed per group.
        aggs: Vec<AggCall>,
        /// [ISO/IEC 9075-2:2023 §7.13 GR2a] The values of groups made before
        /// any row, which are groups even of no rows: of each empty grouping
        /// set (`binder/grouping_sets.rs`).
        seeds: Vec<Row>,
        /// The rows grouped.
        input: Box<LogicalPlan>,
    },
    /// [X3.135-1986 §5.25 GR2] apply the select list.
    Project {
        /// One expression per output column.
        exprs: Vec<Expr>,
        /// The rows projected.
        input: Box<LogicalPlan>,
    },
    /// [X3.135-1986 §5.25 GR2] eliminate redundant duplicate rows.
    Distinct {
        /// The rows.
        input: Box<LogicalPlan>,
    },
    /// [ISO/IEC 9075:1992 §7.5 GR1c, GR2-GR5] An outer join: the rows of
    /// the product of `left` and `right` for which `cond` is true, and
    /// those of the preserved operands that are in none of them, the other
    /// operand's columns null.
    OuterJoin {
        /// LEFT, RIGHT or FULL.
        kind: OuterJoin,
        /// The `<join condition>`, over the product's columns.
        cond: Cond,
        /// The first operand: its columns come first.
        left: Box<LogicalPlan>,
        /// The second operand.
        right: Box<LogicalPlan>,
    },
    /// [X3.135-1986 §8.3 GR2b; ISO/IEC 9075:1992 §7.10 GR1b] UNION,
    /// EXCEPT or INTERSECT \[ALL\], of operands of one degree and types.
    SetOp {
        /// The set operator.
        op: SetOp,
        /// ALL, which keeps duplicates.
        all: bool,
        /// How many leading columns rows are compared by, if not all: their
        /// sort keys, the rows' own values following them (RULES.md D60).
        compared: Option<usize>,
        /// The first operand, which names the columns.
        left: Box<LogicalPlan>,
        /// The second operand.
        right: Box<LogicalPlan>,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] A system table's rows, made
    /// as the statement runs (`system.rs`).
    System {
        /// The table, and whom its rows are made for.
        read: SystemRead,
        /// Its columns' display names.
        names: Vec<String>,
    },
    /// [ISO/IEC 9075:1992 §7.2] A `<table value constructor>`: a row for
    /// each list of expressions, over no row.
    Values {
        /// The rows' expressions.
        rows: Vec<Vec<Expr>>,
        /// The type of each column [§7.2 GR2], as many as each row's degree.
        types: Vec<DataType>,
    },
    /// [X3.135-1986 §8.3 GR3b] ORDER BY.
    Sort {
        /// The sort keys, most significant first.
        keys: Vec<SortKey>,
        /// The rows sorted.
        input: Box<LogicalPlan>,
    },
    /// [ISO/IEC 9075-2:2023 §7.17 GR2c ii] The rows of member `member` of
    /// a recursive stratum of a `<with list>`: those of the fixpoint of its
    /// members' queries.
    Recursive {
        /// Which member's rows, by its place among them.
        member: usize,
        /// The members' queries, and how their fixpoint is found.
        fixpoint: Fixpoint<LogicalPlan>,
        /// Its columns' display names.
        names: Vec<String>,
    },
    /// [ISO/IEC 9075-2:2023 §7.17 GR2c ii 2] A reference, in a query of a
    /// recursive stratum, to its member `member`: the rows the iteration
    /// before gave it, of the `Recursive` it is read in.
    WorkTable {
        /// Which member, by its place among them.
        member: usize,
        /// Its columns' display names.
        names: Vec<String>,
    },
    /// [ISO/IEC 9075-2:2023 §7.6 SR8] A `<collection derived table>`
    /// laterally over `input`: for each of its rows, that row and the
    /// elements of its collections side by side, a row for each ordinal
    /// up to the greatest cardinality, an element past a collection's
    /// cardinality null.
    Unnest(Box<Unnest<LogicalPlan>>),
    /// [ISO/IEC 9075-2:2023 §7.15] The window functions of one window
    /// partitioning and ordering over the rows of `input`.
    Window(Box<Window<LogicalPlan>>),
}

/// [ISO/IEC 9075-2:2023 §7.6 SR8] A `<collection derived table>` over the
/// rows of `input`, of plans `P`: each row of it is a row of `input`, then
/// one column for each element, or one for each field of an element of
/// row type, then the ordinal if WITH ORDINALITY is written.
#[derive(Debug, Clone)]
pub struct Unnest<P> {
    /// The `<collection value expression>`s, over the columns of `input`.
    pub collections: Vec<Expr>,
    /// Of each collection, the degree of its element type if it is a row
    /// type, whose fields are columns of their own [SR8m ii, SR8n].
    pub spread: Vec<Option<usize>>,
    /// WITH ORDINALITY: the ordinal is a column after the elements.
    pub ordinality: bool,
    /// [ISO/IEC 9075-2:2023 §7.10 GR1d] Of the second operand of a LEFT
    /// join, its `<join condition>`, over the columns of its rows: a row of
    /// `input` that none of them makes true is kept, its own columns null.
    pub outer: Option<Cond>,
    /// Its own columns' display names, after `input`'s.
    pub names: Vec<String>,
    /// The rows each gives the rows of its collections.
    pub input: P,
}

impl<P> Unnest<P> {
    /// It apart from its input, and its input.
    pub fn split(self) -> (Unnest<()>, P) {
        let Unnest { collections, spread, ordinality, outer, names, input } = self;
        (Unnest { collections, spread, ordinality, outer, names, input: () }, input)
    }

    /// It over `f` of its input, the rest copied.
    pub fn with_input<Q>(&self, f: impl FnOnce(&P) -> Q) -> Unnest<Q> {
        Unnest {
            collections: self.collections.clone(),
            spread: self.spread.clone(),
            ordinality: self.ordinality,
            outer: self.outer.clone(),
            names: self.names.clone(),
            input: f(&self.input),
        }
    }

    /// The number of its own columns: those of the elements, and the
    /// ordinal's.
    pub fn width(&self) -> usize {
        let elements = self.spread.iter().map(|s| s.unwrap_or(1)).sum::<usize>();
        elements.saturating_add(usize::from(self.ordinality))
    }
}

impl Unnest<()> {
    /// The same collection derived table over `input`.
    pub fn over<Q>(self, input: Q) -> Unnest<Q> {
        let Unnest { collections, spread, ordinality, outer, names, input: () } = self;
        Unnest { collections, spread, ordinality, outer, names, input }
    }
}

/// [ISO/IEC 9075:1992 §7.10] A set operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOp {
    /// UNION: the rows of either.
    Union,
    /// EXCEPT: the rows of the first not in the second.
    Except,
    /// INTERSECT: the rows of both.
    Intersect,
}

/// [ISO/IEC 9075:1992 §7.5] Which operands of an outer join are preserved:
/// their rows that match none are kept \[GR5\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OuterJoin {
    /// LEFT: the first's.
    Left,
    /// RIGHT: the second's.
    Right,
    /// FULL: both's. UNION JOIN is a FULL one on a condition never true.
    Full,
}

impl OuterJoin {
    /// Whether the first operand's rows are preserved.
    pub fn keeps_left(self) -> bool {
        matches!(self, OuterJoin::Left | OuterJoin::Full)
    }

    /// Whether the second operand's rows are preserved.
    pub fn keeps_right(self) -> bool {
        matches!(self, OuterJoin::Right | OuterJoin::Full)
    }
}

/// Adds to `out` the subqueries of `e`, those of its conditions among
/// them, not those nested in them.
fn expr_subqueries<'e>(e: &'e Expr, out: &mut Vec<&'e Subquery>) {
    match e {
        Expr::Subquery(s, _) => out.push(s),
        Expr::Case(whens, _) => out.extend(whens.iter().flat_map(|(c, _)| c.subqueries())),
        Expr::Truth(c) => out.extend(c.subqueries()),
        // A domain's constraints are run for the value CAST, and read their tables.
        Expr::Domain(_, d) => d.checks.iter().for_each(|(_, c)| out.extend(cond_subqueries(c))),
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Cast(..)
        | Expr::SortKey(..)
        | Expr::NextValue(_) => {}
    }
    e.for_each_child(&mut |child| expr_subqueries(child, out));
}

/// The subqueries of `c`, those of its expressions among them, not those
/// nested in them.
fn cond_subqueries(c: &Cond) -> Vec<&Subquery> {
    let mut out = c.subqueries();
    c.for_each_expr(&mut |e| expr_subqueries(e, &mut out));
    out
}

impl Cond {
    /// Whether it has a subquery, among its expressions' too.
    pub fn has_subquery(&self) -> bool {
        !cond_subqueries(self).is_empty()
    }

    /// Every base table the subqueries of this condition read, none if it
    /// has none.
    pub fn tables(&self) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        for s in cond_subqueries(self) {
            s.plan.collect_tables(&mut out);
        }
        out
    }
}

impl Expr {
    /// Every base table the subqueries of this expression read, none if it
    /// has none.
    pub fn tables(&self) -> Vec<QualifiedName> {
        let (mut subqueries, mut out) = (Vec::new(), Vec::new());
        expr_subqueries(self, &mut subqueries);
        for s in subqueries {
            s.plan.collect_tables(&mut out);
        }
        out
    }
}

impl LogicalPlan {
    /// Every base table the subqueries of this plan read, at any node.
    pub fn subquery_tables(&self) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        for s in self.subqueries() {
            s.plan.collect_tables(&mut out);
        }
        for input in self.inputs() {
            out.extend(input.subquery_tables());
        }
        out
    }

    /// The subqueries of this node's own expressions and conditions, not
    /// those of its inputs.
    pub fn subqueries(&self) -> Vec<&Subquery> {
        let mut out = Vec::new();
        match self {
            LogicalPlan::Filter { cond, .. } | LogicalPlan::OuterJoin { cond, .. } => out = cond_subqueries(cond),
            LogicalPlan::Project { exprs, .. } => exprs.iter().for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Aggregate { aggs, .. } => {
                for a in aggs {
                    a.exprs().into_iter().for_each(|e| expr_subqueries(e, &mut out));
                }
            }
            LogicalPlan::Values { rows, .. } => rows.iter().flatten().for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Unnest(u) => {
                u.collections.iter().for_each(|e| expr_subqueries(e, &mut out));
                out.extend(u.outer.iter().flat_map(cond_subqueries));
            }
            LogicalPlan::Window(w) => w.exprs().for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Scan { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. } => {}
        }
        out
    }
}

impl LogicalPlan {
    /// Its inputs, in order: two, one or none, or a recursive stratum's
    /// members' queries, given without a list made.
    pub fn inputs(&self) -> impl Iterator<Item = &LogicalPlan> {
        let (first, second, rest): (Option<&LogicalPlan>, Option<&LogicalPlan>, &[LogicalPlan]) = match self {
            LogicalPlan::Scan { .. } | LogicalPlan::System { .. } | LogicalPlan::Values { .. } | LogicalPlan::WorkTable { .. } => {
                (None, None, &[])
            }
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } | LogicalPlan::SetOp { left, right, .. } => {
                (Some(left), Some(right), &[])
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Project { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Sort { input, .. } => (Some(input), None, &[]),
            LogicalPlan::Unnest(u) => (Some(&u.input), None, &[]),
            LogicalPlan::Window(w) => (Some(&w.input), None, &[]),
            LogicalPlan::Recursive { fixpoint, .. } => fixpoint.plans(),
        };
        first.into_iter().chain(second).chain(rest)
    }

    /// Display names for the columns of this node's output.
    pub fn names(&self) -> Vec<String> {
        match self {
            LogicalPlan::Scan { names, .. }
            | LogicalPlan::System { names, .. }
            | LogicalPlan::Recursive { names, .. }
            | LogicalPlan::WorkTable { names, .. } => names.clone(),
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => {
                let mut n = left.names();
                n.extend(right.names());
                n
            }
            LogicalPlan::Aggregate { groups, aggs, input, .. } => {
                let inner = input.names();
                let mut n: Vec<String> = groups.iter().map(|&g| column_name(&inner, g)).collect();
                n.extend(aggs.iter().map(|a| a.label.clone()));
                n
            }
            LogicalPlan::Project { exprs, input } => {
                let inner = input.names();
                exprs.iter().map(|e| render_expr(e, &inner)).collect()
            }
            LogicalPlan::SetOp { left, .. } => left.names(),
            LogicalPlan::Unnest(u) => {
                let mut n = u.input.names();
                n.extend(u.names.iter().cloned());
                n
            }
            LogicalPlan::Window(w) => {
                let mut n = w.input.names();
                n.extend(w.names());
                n
            }
            LogicalPlan::Values { types, .. } => (0..types.len()).map(|i| column_name(&[], i)).collect(),
            LogicalPlan::Filter { input, .. } | LogicalPlan::Distinct { input } | LogicalPlan::Sort { input, .. } => input.names(),
        }
    }

    /// Every base table this plan reads, its subqueries' among them.
    pub fn tables(&self) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        self.collect_tables(&mut out);
        out
    }

    fn collect_tables(&self, out: &mut Vec<QualifiedName>) {
        if let LogicalPlan::Scan { table, .. } = self
            && !out.contains(table)
        {
            out.push(table.clone());
        }
        for input in self.inputs() {
            input.collect_tables(out);
        }
        // A subquery's tables are read too.
        for s in self.subqueries() {
            s.plan.collect_tables(out);
        }
    }

    /// The number of columns of this node's output.
    pub fn width(&self) -> usize {
        self.names().len()
    }

    /// Rebuild this node with each input replaced by `f(input)`.
    #[must_use]
    pub fn map_inputs(self, mut f: impl FnMut(LogicalPlan) -> LogicalPlan) -> LogicalPlan {
        let mut g = |b: Box<LogicalPlan>| Box::new(f(*b));
        match self {
            LogicalPlan::Scan { .. } | LogicalPlan::System { .. } | LogicalPlan::Values { .. } | LogicalPlan::WorkTable { .. } => self,
            LogicalPlan::Recursive { member, fixpoint, names } => LogicalPlan::Recursive { member, fixpoint: fixpoint.map(f), names },
            LogicalPlan::Filter { cond, input } => LogicalPlan::Filter { cond, input: g(input) },
            LogicalPlan::Product { left, right } => {
                let left = g(left);
                LogicalPlan::Product { left, right: g(right) }
            }
            LogicalPlan::OuterJoin { kind, cond, left, right } => {
                let left = g(left);
                LogicalPlan::OuterJoin { kind, cond, left, right: g(right) }
            }
            LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } => {
                LogicalPlan::Aggregate { groups, collations, aggs, seeds, input: g(input) }
            }
            LogicalPlan::Project { exprs, input } => LogicalPlan::Project { exprs, input: g(input) },
            LogicalPlan::Distinct { input } => LogicalPlan::Distinct { input: g(input) },
            LogicalPlan::SetOp { op, all, compared, left, right } => {
                let left = g(left);
                LogicalPlan::SetOp { op, all, compared, left, right: g(right) }
            }
            LogicalPlan::Sort { keys, input } => LogicalPlan::Sort { keys, input: g(input) },
            LogicalPlan::Unnest(u) => {
                let (u, input) = u.split();
                LogicalPlan::Unnest(Box::new(u.over(f(input))))
            }
            LogicalPlan::Window(w) => {
                let (w, input) = w.split();
                LogicalPlan::Window(Box::new(w.over(f(input))))
            }
        }
    }
}
