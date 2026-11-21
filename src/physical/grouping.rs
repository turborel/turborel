//! The plan of a grouping: its input in the order its groups can give,
//! built for the columns the grouping reads.

use super::{Built, PhysicalPlanner, Plan};
use crate::logical::{AggCall, LogicalPlan, SortKey, column_name, cond_columns, expr_columns};

impl PhysicalPlanner<'_> {
    /// The groups of `logical`, an aggregation, in the order of `want` if
    /// their first rows' order can give it.
    pub(super) fn aggregate(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = logical else { return self.build_node(logical, want) };
        let detail = self.described(input, |names| {
            let seeded = if seeds.is_empty() { String::new() } else { format!(" with {} groups of no rows", seeds.len()) };
            format!(
                "group by [{}] computing [{}]{seeded}",
                groups.iter().map(|&g| column_name(names, g)).collect::<Vec<_>>().join(", "),
                aggs.iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", ")
            )
        });
        // Groups come in the order of their first rows (`exec/group.rs`),
        // so rows in order of grouping columns give groups in that order;
        // groups made before any row come first, in no such order.
        let ordered = collations.is_empty() && seeds.is_empty();
        let inner: Vec<SortKey> = want
            .iter()
            .map(|k| groups.get(k.column).filter(|_| ordered).map(|&c| SortKey { column: c, ..*k }))
            .collect::<Option<_>>()
            .unwrap_or_default();
        let read = reads(groups, aggs);
        let b = self.build_input(input, &inner, &read);
        // Of MIN and MAX alone, its input's joins made semi joins where they
        // can be (`physical/blind.rs`, RULES.md D542).
        let unseeded = if seeds.is_empty() { groups.as_slice() } else { &[0] };
        let (reduced, aggs) = self.blind_joins(b.plan, (unseeded, aggs.clone(), &read));
        let (groups, collations, seeds) = (groups.clone(), collations.clone(), seeds.clone());
        let est = self.card(logical).rows;
        let plan = Plan::Aggregate { groups, collations, aggs, seeds, detail, input: Box::new(reduced), est };
        Built { plan, ordered: !inner.is_empty() && b.ordered }
    }
}

/// The columns a grouping reads of its input's rows: those it groups by,
/// and those its set functions' arguments, filters and distributions read.
fn reads(groups: &[usize], aggs: &[AggCall]) -> Vec<usize> {
    let mut reads = groups.to_vec();
    for a in aggs {
        if let Some(arg) = &a.arg {
            expr_columns(arg, &mut reads);
        }
        if let Some(filter) = &a.filter {
            cond_columns(filter, &mut reads);
        }
        if let Some(distribution) = &a.distribution {
            expr_columns(&distribution.fraction, &mut reads);
        }
    }
    reads
}
