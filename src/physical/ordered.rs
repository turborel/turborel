//! \[DESIGN\] The plans of the nodes that ask an order of their input or
//! pass one asked of them down: windows, set operations, projections and
//! sorts.

use super::{Built, PhysicalPlanner, Plan, expression_index, project};
use crate::logical::{Expr, LogicalPlan, SetOp, SortKey, Window, expr_columns, render_expr, render_keys};

impl PhysicalPlanner<'_> {
    /// The window functions `w`, their input sorted by their partitioning,
    /// each column in the order `want` asks of it if it does, then their
    /// ordering, unless the input gives that order. Its rows keep it, so
    /// it gives `want` if `want` is the first of those keys.
    pub(super) fn window(&self, w: &Window<LogicalPlan>, want: &[SortKey]) -> Built {
        let asked = |c: usize| want.iter().find(|k| k.column == c).copied();
        let mut keys: Vec<SortKey> = w.partition.iter().map(|&c| asked(c).unwrap_or_else(|| SortKey::ascending(c))).collect();
        keys.extend(w.order.iter().copied());
        let b = self.build(&w.input, &keys);
        let detail = self.described(&w.input, |names| w.render(names));
        let input = if b.ordered {
            b.plan
        } else {
            let sorting = self.described(&w.input, |names| render_keys(&keys, names));
            Plan::Sort { keys: keys.clone(), detail: sorting, input: Box::new(b.plan), most: None }
        };
        let window = Box::new(w.with_input(|_| ()).over(input));
        let est = self.card(&w.input).rows;
        Built { plan: Plan::Window { window, detail, est }, ordered: !want.is_empty() && keys.starts_with(want) }
    }

    /// [X3.135-1986 §8.3 GR2b] UNION is UNION ALL followed by duplicate
    /// elimination; EXCEPT and INTERSECT keep the first operand's rows in
    /// its order [ISO/IEC 9075:1992 §7.10 GR1b].
    pub(super) fn set_operation(&self, logical: &LogicalPlan, want: &[SortKey]) -> Built {
        let LogicalPlan::SetOp { op, all, compared, left, right } = logical else { return self.build_node(logical, want) };
        let r = self.build(right, &[]).plan;
        // UNION's rows are in no order, so it asks none of its left operand.
        let asked = if *op == SetOp::Union { &[] } else { want };
        let l = self.build(left, asked);
        let est = self.card(logical).rows;
        Self::combined(*op, (*all, *compared, est), l, r)
    }

    /// A projected column that is a plain column reference keeps the input's
    /// order on that column.
    pub(super) fn project(&self, exprs: &[Expr], input: &LogicalPlan, want: &[SortKey]) -> Built {
        let inner: Vec<SortKey> = want
            .iter()
            .map(|k| {
                let column = expression_index::ordered_by(self.db, exprs.get(k.column)?, input)?;
                Some(SortKey { column, ..*k })
            })
            .collect::<Option<_>>()
            .unwrap_or_default();
        let mut reads = Vec::new();
        for e in exprs {
            expr_columns(e, &mut reads);
        }
        let b = self.build_input(input, &inner, &reads);
        let detail = self.described(input, |names| exprs.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", "));
        let (exprs, source) = project::collapsed(exprs.to_vec(), b.plan);
        Built { plan: Plan::Project { exprs, detail, input: Box::new(source) }, ordered: !inner.is_empty() && b.ordered }
    }

    /// The keys are asked of the input; if it gives them, no Sort is needed.
    pub(super) fn sort(&self, keys: &[SortKey], input: &LogicalPlan) -> Built {
        let b = self.nearest(keys, input).map_or_else(|| self.read_for_sort(input, keys), Built::unordered);
        if b.ordered {
            return Built::unordered(b.plan);
        }
        let detail = self.described(input, |names| render_keys(keys, names));
        Built::unordered(Plan::Sort { keys: keys.to_vec(), detail, input: Box::new(b.plan), most: None })
    }

    /// \[DESIGN\] (RULES.md D1218) `input` asked the order `keys` of a
    /// sort over it. Read in that order, it gives the sort's first rows
    /// as its own, so a row goal holds of it. Read in any other, the sort
    /// reads every row of it before it gives one, so it is planned again
    /// with no goal (`goals.rs`), which a plan made for a few first rows
    /// would be wrong of.
    fn read_for_sort(&self, input: &LogicalPlan, keys: &[SortKey]) -> Built {
        let b = self.build(input, keys);
        if b.ordered || self.first.is_none() { b } else { self.whole().build(input, keys) }
    }
}
