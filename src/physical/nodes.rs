//! \[DESIGN\] The physical plans of single logical nodes, each built apart
//! from `build_node`, which only chooses among them. A walk of the plan
//! recurs through `build_node` once for each level, and a debug build
//! gives every plan value a function makes a stack slot of its own, so
//! the frame it recurs through holds none: a statement nested to
//! `MAX_NESTING_DEPTH` is planned within half the default stack (RULES.md
//! R-CF-6).

use super::{Built, PhysicalPlanner, Plan, of_left};
use crate::functions::XmlFilter;
use crate::logical::{Cond, Expr, Fixpoint, Limit, LogicalPlan, SetOp, SortKey, Unnest, render_expr};
use crate::system::SystemRead;

impl PhysicalPlanner<'_> {
    /// The nested loop of `left` and `right`, which yields rows in the
    /// order of its left input.
    pub(super) fn product(&self, left: &LogicalPlan, right: &LogicalPlan, want: &[SortKey]) -> Built {
        let want_left = of_left(want, left.width());
        let l = self.build_node(left, want_left);
        let r = self.build_node(right, &[]);
        Built { plan: Plan::Product { left: Box::new(l.plan), right: Box::new(r.plan) }, ordered: !want_left.is_empty() && l.ordered }
    }

    /// Duplicate elimination of `input`, which keeps the first occurrence,
    /// so order is kept.
    pub(super) fn distinct(&self, input: &LogicalPlan, want: &[SortKey]) -> Built {
        let b = self.build(input, want);
        Built { plan: Plan::Distinct { input: Box::new(b.plan) }, ordered: b.ordered }
    }

    /// A system table's rows.
    pub(super) fn system(read: &SystemRead) -> Built {
        Built::unordered(Plan::System { read: read.clone() })
    }

    /// A table value constructor's rows, shown by EXPLAIN if it shows
    /// details.
    pub(super) fn values(&self, rows: &[Vec<Expr>]) -> Built {
        let row = |r: &Vec<Expr>| format!("({})", r.iter().map(|e| render_expr(e, &[])).collect::<Vec<_>>().join(", "));
        let detail = if self.shown { rows.iter().map(row).collect::<Vec<_>>().join(", ") } else { String::new() };
        Built::unordered(Plan::Values { rows: rows.to_vec(), detail })
    }

    /// OFFSET and FETCH FIRST of `input`. It keeps its input's order, the
    /// one `want` asks of it if it gives that: of no ORDER BY, which rows
    /// it keeps is implementation-dependent [ISO/IEC 9075-2:2023 §7.17
    /// GR5a]. Its input is planned knowing how many of its first rows
    /// are read, if it is known.
    pub(super) fn limited(&self, limit: &Limit, input: &LogicalPlan, want: &[SortKey]) -> Built {
        let b = PhysicalPlanner { first: limit.read_rows(), fetch: limit.fetch.as_ref(), ..*self }.build(input, want);
        Built { plan: Plan::Limit { limit: limit.clone(), input: Box::new(b.plan) }, ordered: b.ordered }
    }

    /// The rows of member `member` of a recursive stratum, its members'
    /// queries each planned.
    pub(super) fn recursive(&self, member: usize, fixpoint: &Fixpoint<LogicalPlan>) -> Built {
        Built::unordered(Plan::Recursive { member, fixpoint: fixpoint.map_ref(|p| self.plan(p)) })
    }

    /// The rows the iteration before gave member `member`.
    pub(super) fn work_table(member: usize) -> Built {
        Built::unordered(Plan::WorkTable { member, rows: None })
    }

    /// A collection derived table over its input, planned: of XMLTABLE
    /// joined to the rows of a table, its input filtered by its row
    /// pattern, implied (`implied_filter`).
    pub(super) fn unnest(&self, u: &Unnest<LogicalPlan>) -> Built {
        let filtered = implied_filter(u);
        let input = self.plan(filtered.as_deref().unwrap_or(&u.input));
        Built::unordered(Plan::Unnest(Box::new(u.with_input(|_| input))))
    }

    /// The set operation `op` of the plans `left`, whose order is kept,
    /// and `right`: UNION ALL an append of them, UNION one without
    /// duplicates, in no order.
    pub(super) fn combined(op: SetOp, (all, compared): (bool, Option<usize>), left: Built, right: Plan) -> Built {
        let (l, r) = (Box::new(left.plan), Box::new(right));
        if op != SetOp::Union {
            return Built { plan: Plan::SetOp { op, all, compared, left: l, right: r }, ordered: left.ordered };
        }
        let append = Plan::Append { left: l, right: r };
        Built::unordered(if all { append } else { Plan::Distinct { input: Box::new(append) } })
    }
}

/// \[EXTENSION\] RULES.md D106: of `u`, XMLTABLE of a row pattern of the
/// context item alone, joined to the rows of a table it is of, not LEFT,
/// the table read with its row pattern as an implied filter
/// (`functions::XmlRowsOf`): a row it gives the empty sequence of gives no
/// row, so an XML index may find the others, each checked by XMLTABLE.
fn implied_filter(u: &Unnest<LogicalPlan>) -> Option<Box<LogicalPlan>> {
    let [Expr::Call(f, args)] = u.collections.as_slice() else { return None };
    if u.outer.is_some() {
        return None;
    }
    let XmlFilter::NoRows(_, rows) = f.0.xml_filter()? else { return None };
    // A table's own filter stays above the collection derived table.
    if !matches!(u.input, LogicalPlan::Scan { .. }) {
        return None;
    }
    let cond = Cond::Value(Expr::Call(rows, vec![args.first()?.clone()]));
    Some(Box::new(LogicalPlan::Filter { cond, input: Box::new(u.input.clone()) }))
}
