//! A window's rows narrowed to the columns that hold a value
//! (`blind/pruned.rs`, RULES.md D1245). Pruned, a window's input is a
//! projection that gives null for each column nothing reads, so a row
//! still has a place for every column, as has a row of a scan that reads
//! some of its table's columns. A window run a row at a time holds,
//! sorts, splits and spills its rows whole, nulls and all. So a window
//! under a projection, whose input is such a projection or scan, under a
//! sort or not, is given the columns that hold a value alone: the
//! projection under it keeps those, or one of them is put over the scan,
//! and its partitioning, its ordering, its calls, the sort's keys and the
//! projection above it are numbered anew. The projection above gives what it gave, so nothing above it
//! changes. Any other window keeps its nulls, which cost nothing in a
//! batch's vectors.

use super::super::super::{PhysicalPlanner, Plan};
use super::windowed::window_reads;
use crate::logical::{Expr, Window, expr_columns, map_expr_columns};
use crate::value::Value;

impl PhysicalPlanner<'_> {
    /// `plan` with each window under a projection narrowed so. A function
    /// of little on the stack, as the walk is as deep as the plan.
    pub(super) fn narrowed(&self, plan: &mut Plan) {
        plan.each_input(&mut |input| self.narrowed(input));
        self.narrowed_here(plan);
    }

    /// `plan` narrowed, if it is a projection of a window that can be.
    #[inline(never)]
    fn narrowed_here(&self, plan: &mut Plan) {
        let Plan::Project { exprs: above, input, .. } = plan else { return };
        let Plan::Window { window, .. } = &mut **input else { return };
        let Some((live, width)) = self.narrowing(above, window) else { return };
        // A column of its input moves to its place among those held; one
        // of its calls, or one a nested window function gives, after them.
        let to =
            |c: usize| if c < width { live.binary_search(&c).unwrap_or(c) } else { c.saturating_sub(width).saturating_add(live.len()) };
        for e in above.iter_mut() {
            *e = map_expr_columns(std::mem::replace(e, Expr::Const(Value::Null)), &to);
        }
        for c in &mut window.partition {
            *c = to(*c);
        }
        for k in &mut window.order {
            k.column = to(k.column);
        }
        window.map_exprs(&mut |e| map_expr_columns(e, &to));
        self.narrowed_input(&mut window.input, (&live, width), &to);
    }

    /// The columns of `window`'s input that hold a value, in order, and its
    /// input's width, if it can be narrowed under the projection of
    /// `above`: it searches for no row pattern, its input holds no value in
    /// some column, and every column it and `above` read holds one.
    fn narrowing(&self, above: &[Expr], window: &Window<Plan>) -> Option<(Vec<usize>, usize)> {
        if window.calls.iter().any(|c| c.frame.matching.is_some()) {
            return None;
        }
        let (live, width) = self.held(&window.input)?;
        let mut reads = window_reads(window, width, &[]);
        for e in above {
            if !super::single::plain(e) {
                return None;
            }
            expr_columns(e, &mut reads);
        }
        reads.iter().filter(|&&c| c < width).all(|c| live.binary_search(c).is_ok()).then_some((live, width))
    }

    /// The columns of `input`, or of what it sorts, that hold a value, in
    /// order, and its width, if some hold none: of a projection of columns
    /// and nulls, its columns; of a scan that reads some of its table's
    /// columns, those.
    fn held(&self, input: &Plan) -> Option<(Vec<usize>, usize)> {
        let below = if let Plan::Sort { input, .. } = input { &**input } else { input };
        let (live, width) = if let Plan::Project { exprs, .. } = below {
            let nulls = |e: &Expr| if let Expr::Const(v) = e { v.is_null() } else { false };
            if !exprs.iter().all(|e| a_column(e) || nulls(e)) {
                return None;
            }
            (exprs.iter().enumerate().filter(|(_, e)| a_column(e)).map(|(c, _)| c).collect(), exprs.len())
        } else if let Plan::SeqScan { columns, located: false, .. } = below {
            let mut live = columns.clone();
            live.sort_unstable();
            live.dedup();
            (live, self.sized(below).width?)
        } else {
            return None;
        };
        (live.len() < width).then_some((live, width))
    }

    /// `input`, a window's projection of columns and nulls or scan of some
    /// columns, or a sort of either, of the columns `live` that hold a
    /// value of its `width`, its keys numbered by `to`: the projection
    /// keeping its columns alone, or the scan put under a projection of
    /// them, named so.
    fn narrowed_input(&self, input: &mut Plan, (live, width): (&[usize], usize), to: &dyn Fn(usize) -> usize) {
        if let Plan::Sort { keys, input: below, .. } = input {
            for k in keys.iter_mut() {
                k.column = to(k.column);
            }
            self.narrowed_input(below, (live, width), to);
        } else if let Plan::Project { exprs, detail, .. } = input {
            let pieces: Vec<&str> = detail.split(", ").collect();
            if pieces.len() == exprs.len() {
                let kept: Vec<&str> = pieces.into_iter().zip(exprs.iter()).filter(|(_, e)| a_column(e)).map(|(p, _)| p).collect();
                *detail = kept.join(", ");
            }
            exprs.retain(a_column);
        } else if matches!(input, Plan::SeqScan { .. }) {
            let names = self.names(input, width);
            let detail = live.iter().filter_map(|&c| names.get(c).cloned()).collect::<Vec<_>>().join(", ");
            let exprs = live.iter().map(|&c| Expr::Column(c)).collect();
            let scan = Box::new(std::mem::replace(input, super::placeholder()));
            *input = Plan::Project { exprs, detail, input: scan };
        }
    }
}

/// Whether `e` moves a column.
fn a_column(e: &Expr) -> bool {
    matches!(e, Expr::Column(_))
}
