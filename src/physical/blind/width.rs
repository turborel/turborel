//! The widths of a physical plan's rows, of each operator the reduction
//! walks through (`physical/blind.rs`), found once, from the scans up.

use super::super::{PhysicalPlanner, Plan, Preserved};

/// The width of an operator's rows, if known here, and those of its
/// inputs the reduction walks into, in their order.
pub(super) struct Sized {
    pub(super) width: Option<usize>,
    kids: Vec<Sized>,
}

/// The widths of an operator the reduction does not walk into.
static UNKNOWN: Sized = Sized { width: None, kids: Vec::new() };

impl Sized {
    /// The widths of its input `n`.
    pub(super) fn kid(&self, n: usize) -> &Sized {
        self.kids.get(n).unwrap_or(&UNKNOWN)
    }

    /// The width of its input `n`.
    pub(super) fn of(&self, n: usize) -> Option<usize> {
        self.kid(n).width
    }
}

/// The sum of two widths, if both are known.
fn sum(a: Option<usize>, b: Option<usize>) -> Option<usize> {
    Some(a?.saturating_add(b?))
}

impl PhysicalPlanner<'_> {
    /// The widths of `plan` and of its inputs: a scan's, its table's and
    /// its row id's; a join's, its operands'; a semi join's, its left's.
    pub(super) fn sized(&self, plan: &Plan) -> Sized {
        let table = |name| self.db.catalog.table(name).map(|t| t.columns.len());
        let alone = |width| Sized { width, kids: Vec::new() };
        match plan {
            Plan::SeqScan { table: name, located, .. } | Plan::IndexScan { table: name, located, .. } => {
                alone(table(name).map(|w| w.saturating_add(usize::from(*located))))
            }
            Plan::Filter { input, .. } => {
                let kid = self.sized(input);
                Sized { width: kid.width, kids: vec![kid] }
            }
            Plan::Project { exprs, input, .. } => Sized { width: Some(exprs.len()), kids: vec![self.sized(input)] },
            Plan::HashJoin { left, right, .. } | Plan::Product { left, right, .. } => {
                let (l, r) = (self.sized(left), self.sized(right));
                Sized { width: sum(l.width, r.width), kids: vec![l, r] }
            }
            Plan::IndexJoin { outer, left, width, .. } => {
                let kid = self.sized(left);
                let found = if matches!(outer, Preserved::Semi | Preserved::Anti) { 0 } else { *width };
                Sized { width: kid.width.map(|w| w.saturating_add(found)), kids: vec![kid] }
            }
            Plan::SemiJoin(s) => alone(Some(s.widths.0)),
            Plan::Nearest(_)
            | Plan::Counted(_)
            | Plan::OuterJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::RangeJoin(_)
            | Plan::MarkJoin(_)
            | Plan::Aggregate { .. }
            | Plan::Distinct { .. }
            | Plan::SetOp { .. }
            | Plan::System { .. }
            | Plan::Values { .. }
            | Plan::Append { .. }
            | Plan::MergeAppend { .. }
            | Plan::Sort { .. }
            | Plan::Limit { .. }
            | Plan::Recursive { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Unnest(_)
            | Plan::Window { .. }
            | Plan::Recognize { .. }
            | Plan::Polymorphic(_)
            | Plan::Parallel(_) => alone(None),
        }
    }
}
