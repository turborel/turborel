//! \[DESIGN\] The logical plan: relational algebra over bound expressions.
//!
//! It says *what* to compute and knows nothing about storage, so rewrite
//! rules (optimizer.rs) can work on it without knowing which engine, index or
//! layout will be used. The physical planner (physical.rs) decides *how*.
//!
//! Columns are positions. Each node's output row is described by `names()`:
//! a Scan outputs its table's columns, a Product the left's then the right's,
//! an Aggregate its grouping columns then one column per set function.

mod expr;
mod render;
mod subquery;

use std::fmt::Write;

pub use expr::{
    Cond, Expr, Quantifier, Truth, and_all, cond_columns, expr_columns, map_cond_columns, map_cond_exprs, map_expr_columns, or_all,
    split_and,
};
pub use render::{column_name, render_cond, render_expr, render_keys};
pub use subquery::{Decorrelated, Subquery};

use crate::catalog::QualifiedName;
use crate::functions::AggregateRef;
use crate::storage::Row;
use crate::system::SystemRead;
use crate::value::DataType;

/// One set function to compute per group.
#[derive(Debug, Clone)]
pub struct AggCall {
    /// The set function.
    pub func: AggregateRef,
    /// None for COUNT(*); otherwise evaluated against each row of the group.
    pub arg: Option<Expr>,
    /// Whether duplicate values are removed first [X3.135-1986 §5.8 GR1].
    pub distinct: bool,
    /// The type of the result.
    pub result: DataType,
    /// The call as written, e.g. "COUNT(DISTINCT E.DEPT)": its column name and
    /// how repeats of one call are found.
    pub label: String,
}

/// [X3.135-1986 §8.3 GR3b] a sort column of the node's output, and its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    /// The column's position in the node's output.
    pub column: usize,
    /// DESC rather than ASC.
    pub descending: bool,
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
        /// The set functions computed per group.
        aggs: Vec<AggCall>,
        /// The rows grouped.
        input: Box<LogicalPlan>,
    },
    /// [X3.135-1986 §5.25 GR3] one row of set-function results if the input has no rows.
    DefaultRow {
        /// The row given for no groups.
        row: Row,
        /// The grouped rows.
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
        /// The degree of each.
        width: usize,
    },
    /// [X3.135-1986 §8.3 GR3b] ORDER BY.
    Sort {
        /// The sort keys, most significant first.
        keys: Vec<SortKey>,
        /// The rows sorted.
        input: Box<LogicalPlan>,
    },
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
        Expr::Column(_) | Expr::Const(_) | Expr::Neg(_) | Expr::Arith(..) | Expr::Call(..) | Expr::Outer(..) | Expr::Cast(..) => {}
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
            LogicalPlan::Aggregate { aggs, .. } => aggs.iter().filter_map(|a| a.arg.as_ref()).for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Values { rows, .. } => rows.iter().flatten().for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Scan { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::DefaultRow { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Sort { .. } => {}
        }
        out
    }
}

impl LogicalPlan {
    /// Its inputs, in order.
    pub fn inputs(&self) -> Vec<&LogicalPlan> {
        match self {
            LogicalPlan::Scan { .. } | LogicalPlan::System { .. } | LogicalPlan::Values { .. } => Vec::new(),
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } | LogicalPlan::SetOp { left, right, .. } => {
                vec![left, right]
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::DefaultRow { input, .. }
            | LogicalPlan::Project { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Sort { input, .. } => vec![input],
        }
    }

    /// Display names for the columns of this node's output.
    pub fn names(&self) -> Vec<String> {
        match self {
            LogicalPlan::Scan { names, .. } | LogicalPlan::System { names, .. } => names.clone(),
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => {
                let mut n = left.names();
                n.extend(right.names());
                n
            }
            LogicalPlan::Aggregate { groups, aggs, input } => {
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
            LogicalPlan::Values { width, .. } => (0..*width).map(|i| column_name(&[], i)).collect(),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::DefaultRow { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Sort { input, .. } => input.names(),
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
            LogicalPlan::Scan { .. } | LogicalPlan::System { .. } | LogicalPlan::Values { .. } => self,
            LogicalPlan::Filter { cond, input } => LogicalPlan::Filter { cond, input: g(input) },
            LogicalPlan::Product { left, right } => {
                let left = g(left);
                LogicalPlan::Product { left, right: g(right) }
            }
            LogicalPlan::OuterJoin { kind, cond, left, right } => {
                let left = g(left);
                LogicalPlan::OuterJoin { kind, cond, left, right: g(right) }
            }
            LogicalPlan::Aggregate { groups, aggs, input } => LogicalPlan::Aggregate { groups, aggs, input: g(input) },
            LogicalPlan::DefaultRow { row, input } => LogicalPlan::DefaultRow { row, input: g(input) },
            LogicalPlan::Project { exprs, input } => LogicalPlan::Project { exprs, input: g(input) },
            LogicalPlan::Distinct { input } => LogicalPlan::Distinct { input: g(input) },
            LogicalPlan::SetOp { op, all, left, right } => {
                let left = g(left);
                LogicalPlan::SetOp { op, all, left, right: g(right) }
            }
            LogicalPlan::Sort { keys, input } => LogicalPlan::Sort { keys, input: g(input) },
        }
    }

    /// The plan as indented text, one node per line.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        let inner = |p: &LogicalPlan| p.names();
        // Writing to a String cannot fail.
        let _ = match self {
            LogicalPlan::Scan { table, .. } => writeln!(out, "{pad}Scan {table}"),
            LogicalPlan::Filter { cond, input } => writeln!(out, "{pad}Filter {}", render_cond(cond, &inner(input))),
            LogicalPlan::Product { .. } => writeln!(out, "{pad}Product"),
            LogicalPlan::OuterJoin { kind, cond, .. } => {
                writeln!(out, "{pad}{kind:?} outer join on {}", render_cond(cond, &self.names()))
            }
            LogicalPlan::Aggregate { groups, aggs, input } => {
                let n = inner(input);
                let g: Vec<String> = groups.iter().map(|&i| column_name(&n, i)).collect();
                let a: Vec<&str> = aggs.iter().map(|a| a.label.as_str()).collect();
                writeln!(out, "{pad}Aggregate group by [{}] computing [{}]", g.join(", "), a.join(", "))
            }
            LogicalPlan::DefaultRow { .. } => writeln!(out, "{pad}DefaultRow"),
            LogicalPlan::Project { exprs, input } => {
                let n = inner(input);
                writeln!(out, "{pad}Project {}", exprs.iter().map(|e| render_expr(e, &n)).collect::<Vec<_>>().join(", "))
            }
            LogicalPlan::Distinct { .. } => writeln!(out, "{pad}Distinct"),
            LogicalPlan::SetOp { op, all, .. } => writeln!(out, "{pad}{op:?}{}", if *all { " all" } else { "" }),
            LogicalPlan::System { read, .. } => writeln!(out, "{pad}System {}", read.name()),
            LogicalPlan::Values { rows, .. } => writeln!(
                out,
                "{pad}Values {}",
                rows.iter()
                    .map(|r| format!("({})", r.iter().map(|e| render_expr(e, &[])).collect::<Vec<_>>().join(", ")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            LogicalPlan::Sort { keys, input } => writeln!(out, "{pad}Sort {}", render_keys(keys, &inner(input))),
        };
        for input in self.inputs() {
            input.write(out, depth.saturating_add(1));
        }
        // Its subqueries, each run for a row with its outer references'
        // values.
        for s in self.subqueries() {
            let _ = writeln!(out, "{pad}  Subquery");
            s.plan.write(out, depth.saturating_add(2));
        }
    }
}
