//! Whether a plan can raise an error of a General Rule as it runs.

use super::Plan;
use crate::logical::{Cond, Expr};

impl Plan {
    /// Whether running it can raise an error of a General Rule (an
    /// overflow, a division by zero, a subquery's cardinality): through a
    /// condition or expression that can, or a set function, which can
    /// overflow. Reading a table can fail too, but that is the storage's
    /// failure, not the statement's.
    pub fn can_raise(&self) -> bool {
        match self {
            Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::WorkTable { .. } | Plan::Transition { .. } => false,
            // Past EXECUTOR.MAX_ITERATIONS a recursive query is refused,
            // a system table may fail to make its rows, and a count not a
            // row count raises [ISO/IEC 9075-2:2023 §7.17 GR6b, GR7c]; a
            // polymorphic table function's component procedure may raise
            // any exception.
            Plan::Recursive { .. } | Plan::System { .. } | Plan::Limit { .. } | Plan::Polymorphic(_) | Plan::Nearest(_) => true,
            Plan::Filter { cond, input, .. } => cond.can_raise() || input.can_raise(),
            Plan::Parallel(p) => p.input.can_raise(),
            Plan::GroupJoin(g) => !g.aggs.is_empty() || g.build.can_raise() || g.probe.can_raise(),
            Plan::Counted(c) => c.input.can_raise(),
            Plan::Project { exprs, input, .. } => exprs.iter().any(Expr::can_raise) || input.can_raise(),
            Plan::Aggregate { aggs, input, .. } => !aggs.is_empty() || input.can_raise(),
            Plan::Product { left, right, .. }
            | Plan::HashJoin { left, right, .. }
            | Plan::Append { left, right, .. }
            | Plan::SetOp { left, right, .. } => left.can_raise() || right.can_raise(),
            Plan::Values { rows, .. } => rows.iter().flatten().any(Expr::can_raise),
            Plan::MergeJoin(m) => m.left.can_raise() || m.right.can_raise() || m.cond.as_ref().is_some_and(Cond::can_raise),
            Plan::OuterJoin { left, right, cond, own, .. } => {
                let owned = own.as_deref().is_some_and(|(l, r)| [l, r].iter().any(|c| c.as_ref().is_some_and(Cond::can_raise)));
                left.can_raise() || right.can_raise() || owned || cond.as_ref().is_some_and(Cond::can_raise)
            }
            // Spatial adaptation is admitted only for same-SRID geometry
            // columns and non-raising residuals. Intersects has no other
            // General Rule error on those stored values. Decode damage is
            // still a storage failure, as for an ordinary scan (D15, D85).
            Plan::IndexJoin { spatial, outer, left, cond, .. } => {
                let proved = spatial.is_some() && matches!(outer, super::Preserved::Inner);
                left.can_raise() || (!proved && cond.as_ref().is_some_and(Cond::can_raise))
            }
            Plan::Distinct { input, .. } | Plan::Sort { input, .. } => input.can_raise(),
            Plan::MergeAppend { inputs, .. } => inputs.iter().any(Plan::can_raise),
            // A GRAPH_TABLE's search can always pass its limits.
            Plan::Unnest(u) => {
                let own = u.graph.is_some() || u.collections.iter().any(Expr::can_raise);
                own || u.outer.as_ref().is_some_and(Cond::can_raise) || u.input.can_raise()
            }
            Plan::Window { window, .. } => window.can_raise() || window.input.can_raise(),
            // Its search can always pass the matcher's limits.
            Plan::Recognize { recognize, .. } => recognize.can_raise(),
        }
    }
}
