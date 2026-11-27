//! \[DESIGN\] The two groupings of an aggregation of DISTINCT set
//! functions (RULES.md D340, D342), from the bottom:
//!
//! - of more than one copy (`Shape::copies`), each row made once for
//!   each, a marker after its columns, 1 to their number: by reading the
//!   rows again for each (`copies.rs`), or else by a collection derived
//!   table of the markers, so the rows are read once;
//! - of a slot with a filter, the filter's truth of each row, of its own
//!   copy alone, in a projection of the columns used, so it is evaluated
//!   once a row as before [ISO/IEC 9075-2:2023 §10.9 GR4a];
//! - a projection of the columns used, those truths, and each slot's
//!   value: its argument of a row of its copy its filter admits, else
//!   null, so the argument is computed of the rows it was;
//! - the first grouping: by the grouping columns, the marker, the truths
//!   and the values, the functions without DISTINCT computed in parts of
//!   the rows of one copy (`functions/partial.rs`);
//! - the second: by the grouping columns, each DISTINCT function of the
//!   value of its slot, of the groups of its copy its filter admitted,
//!   each other merging the parts of its copy.
//!
//! A null value of a slot, of rows its filter admitted, is a group of
//! the first, so the second takes it out and warns [§10.9 GR7a], as the
//! function did; no other null reaches a function. Groups of the second
//! come in the order of their first rows, as each group's first group of
//! the first comes of its first row. Groups of no rows, of empty
//! grouping sets [§7.13 GR2a], are the second's alone.

use super::{Shape, Slot, copies};
use crate::functions::{AggregateRef, ColumnarKind, Merged, PART_TYPE, PARTIAL, Partial};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, Unnest, expr_columns, map_cond_columns, map_expr_columns};
use crate::storage::Row;
use crate::value::collation::CollationRef;
use crate::value::{CompOp, Decimal, Value};
use std::sync::Arc;

/// The name of the marker's column.
const MARKER: &str = "COPY";

/// What an aggregation groups by, apart from its functions and rows.
pub(super) struct Grouping<'a> {
    pub(super) groups: &'a [usize],
    pub(super) collations: &'a [Option<CollationRef>],
    pub(super) seeds: &'a [Row],
}

/// The aggregation of `grouping` computing `aggs` of `input`.
pub(super) fn regrouped(grouping: &Grouping<'_>, aggs: Vec<AggCall>, input: LogicalPlan) -> LogicalPlan {
    LogicalPlan::Aggregate {
        groups: grouping.groups.to_vec(),
        collations: grouping.collations.to_vec(),
        aggs,
        seeds: grouping.seeds.to_vec(),
        input: Box::new(input),
    }
}

/// The marker `n`, an exact number of scale 0.
pub(super) fn marker(n: usize) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0)))
}

/// Whether the marker of column `at` is `n`.
fn marked(at: usize, n: usize) -> Cond {
    Cond::Compare(Expr::Column(at), CompOp::Eq, marker(n))
}

/// `e` if `when` is true, else null; or the truth `when` alone, false
/// where it is not of its copy.
fn when(guard: Cond, e: Expr, otherwise: Value) -> Expr {
    Expr::Case(vec![(guard, e)].into(), Box::new(Expr::Const(otherwise)))
}

/// Where each column of the plans between the two groupings is.
pub(super) struct Layout {
    /// The columns of the rows (and the marker) used, in order.
    pub(super) used: Vec<usize>,
    /// The marker's column of the rows, if there is more than one copy.
    pub(super) marker: Option<usize>,
    /// The copy whose rows the functions without DISTINCT take.
    pub(super) plain: usize,
    /// Of each slot, its filter's truth's place among the truths.
    pub(super) truths: Vec<Option<usize>>,
    /// The number of grouping columns.
    pub(super) width: usize,
}

impl Layout {
    /// The place of column `c` of the rows among those used.
    fn at(&self, c: usize) -> usize {
        self.used.iter().position(|&u| u == c).unwrap_or(c)
    }

    /// The number of truths.
    fn truth_count(&self) -> usize {
        self.truths.iter().flatten().count()
    }

    /// The column of the first grouping's rows of the marker, after the
    /// grouping columns: 0 or 1 of them.
    fn marked(&self) -> usize {
        usize::from(self.marker.is_some())
    }

    /// The second grouping's condition of the groups of the slot `s`.
    fn admits(&self, s: usize) -> Option<Cond> {
        let truths = self.width.saturating_add(self.marked());
        match (self.truths.get(s).copied().flatten(), self.marker) {
            (Some(t), _) => Some(Cond::Value(Expr::Column(truths.saturating_add(t)))),
            (None, Some(_)) => Some(marked(self.width, s.saturating_add(1))),
            (None, None) => None,
        }
    }
}

/// The two groupings computing `shape` of `input` under `grouping`.
pub(super) fn two_levels(grouping: &Grouping<'_>, shape: &Shape, input: LogicalPlan) -> LogicalPlan {
    let (projected, layout) = copies::appended(grouping.groups, shape, &input).unwrap_or_else(|| {
        let (rows, layout) = laid_out(grouping, shape, input, shape.copies());
        (valued(rows, &layout, &shape.slots), layout)
    });
    let inner = first(grouping, shape, &layout, projected);
    let mut aggs = Vec::new();
    let parts = layout.width.saturating_add(layout.marked()).saturating_add(layout.truth_count()).saturating_add(shape.slots.len());
    let mut part = parts;
    for (a, of) in shape.aggs.iter().zip(&shape.of) {
        aggs.push(if let Some(s) = of {
            let value = Expr::Column(parts.saturating_sub(shape.slots.len()).saturating_add(*s));
            let arg =
                shape.slots.get(*s).and_then(|slot| slot.keyed.as_ref()).map_or_else(|| value.clone(), |c| keyed(a, value.clone(), c));
            AggCall { arg: Some(arg), distinct: false, filter: layout.admits(*s), ..a.clone() }
        } else {
            let func = AggregateRef(Arc::new(Merged { func: a.func.clone(), result: a.result.clone() }));
            let filter = layout.marker.map(|_| marked(layout.width, layout.plain));
            let arg = Some(Expr::Column(part));
            part = part.saturating_add(1);
            AggCall { func, arg, filter, ..a.clone() }
        });
    }
    let groups: Vec<usize> = (0..layout.width).collect();
    regrouped(&Grouping { groups: &groups, ..*grouping }, aggs, inner)
}

/// \[DESIGN\] (RULES.md D1219) What the function `a` takes of `value`, a
/// group's string the first grouping grouped under `collation` in place
/// of its sort key: the key, made once a group, as the function took it
/// before; but the string itself of a function that only counts its
/// values, as a sort key is null exactly when its string is.
fn keyed(a: &AggCall, value: Expr, collation: &CollationRef) -> Expr {
    let counts = a.func.0.columnar().is_some_and(|c| c.0 == ColumnarKind::Count);
    if counts { value } else { Expr::sort_key(value, collation) }
}

/// The rows of `input` made once for each of `copies`, if more than one,
/// and of each slot with a filter its truth; and where their columns are.
fn laid_out(grouping: &Grouping<'_>, shape: &Shape, input: LogicalPlan, copies: usize) -> (LogicalPlan, Layout) {
    let wide = input.width();
    let (rows, marker) = if copies > 1 { (copied(input, copies), Some(wide)) } else { (input, None) };
    let mut used = grouping.groups.to_vec();
    used.extend(marker);
    for (a, of) in shape.aggs.iter().zip(&shape.of) {
        if of.is_none() {
            a.exprs().into_iter().for_each(|e| expr_columns(e, &mut used));
        }
    }
    // The arguments are computed of the rows, but past the truths'
    // projection, which must hold their columns.
    let filtered = shape.slots.iter().any(|s| s.filter.is_some());
    for slot in shape.slots.iter().filter(|_| filtered) {
        expr_columns(&slot.arg, &mut used);
    }
    used.sort_unstable();
    used.dedup();
    let plain = if shape.copies() > shape.slots.len() { copies } else { 1 };
    let mut layout = Layout { used, marker, plain, truths: Vec::new(), width: grouping.groups.len() };
    if !filtered {
        return (rows, layout);
    }
    let mut exprs: Vec<Expr> = layout.used.iter().map(|&c| Expr::Column(c)).collect();
    for (s, slot) in shape.slots.iter().enumerate() {
        let truth = slot.filter.clone().map(|c| Expr::Truth(Box::new(c)));
        let place = truth.map(|t| {
            exprs.push(marker.map_or_else(|| t.clone(), |m| when(marked(m, s.saturating_add(1)), t.clone(), Value::Bool(false))));
            exprs.len().saturating_sub(layout.used.len()).saturating_sub(1)
        });
        layout.truths.push(place);
    }
    (LogicalPlan::Project { exprs, input: Box::new(rows) }, layout)
}

/// The rows of `input`, each made `copies` times, its marker after it.
fn copied(input: LogicalPlan, copies: usize) -> LogicalPlan {
    let markers: Vec<Value> = (1..=copies).map(|n| Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))).collect();
    LogicalPlan::Unnest(Box::new(Unnest {
        collections: vec![Expr::Const(Value::Array(markers.into()))],
        spread: vec![None],
        ordinality: false,
        outer: None,
        names: vec![MARKER.to_owned()],
        input,
        json: None,
        graph: None,
    }))
}

/// The columns used of `rows`, the truths, then each slot's value.
fn valued(rows: LogicalPlan, layout: &Layout, slots: &[Slot]) -> LogicalPlan {
    let truths = layout.truth_count();
    // The truths' projection holds the columns used in their order.
    let column = |c: usize| if truths > 0 { layout.at(c) } else { c };
    let mut exprs: Vec<Expr> = layout.used.iter().map(|&c| Expr::Column(column(c))).collect();
    exprs.extend((0..truths).map(|t| Expr::Column(layout.used.len().saturating_add(t))));
    for (s, slot) in slots.iter().enumerate() {
        let arg = map_expr_columns(slot.arg.clone(), &column);
        let guard = match (layout.truths.get(s).copied().flatten(), layout.marker) {
            (Some(t), _) => Some(Cond::Value(Expr::Column(layout.used.len().saturating_add(t)))),
            (None, Some(m)) => Some(marked(column(m), s.saturating_add(1))),
            (None, None) => None,
        };
        exprs.push(guard.map_or_else(|| arg.clone(), |g| when(g, arg.clone(), Value::Null)));
    }
    LogicalPlan::Project { exprs, input: Box::new(rows) }
}

/// The first grouping, of `projected`: by the grouping columns, the
/// marker, the truths and the values; each function without DISTINCT in
/// parts, of the rows of its copy.
fn first(grouping: &Grouping<'_>, shape: &Shape, layout: &Layout, projected: LogicalPlan) -> LogicalPlan {
    let used = layout.used.len();
    let mut groups: Vec<usize> = grouping.groups.iter().map(|&g| layout.at(g)).collect();
    groups.extend(layout.marker.map(|m| layout.at(m)));
    groups.extend(used..used.saturating_add(layout.truth_count()).saturating_add(shape.slots.len()));
    let mut collations = grouping.collations.to_vec();
    if !collations.is_empty() || shape.slots.iter().any(|slot| slot.keyed.is_some()) {
        collations.resize(groups.len(), None);
    }
    // A slot's string is grouped under the collation its key was of
    // (RULES.md D1219), the slots the last of the grouping columns.
    let slots = groups.len().saturating_sub(shape.slots.len());
    for (by, slot) in collations.iter_mut().skip(slots).zip(&shape.slots) {
        by.clone_from(&slot.keyed);
    }
    let at = |c: usize| layout.at(c);
    let aggs = shape
        .aggs
        .iter()
        .zip(&shape.of)
        .filter(|(_, of)| of.is_none())
        .map(|(a, _)| {
            let func = AggregateRef(Arc::new(Partial { func: a.func.clone(), result: a.result.clone() }));
            let own = a.filter.clone().map(|c| map_cond_columns(c, &at));
            let filter = match (layout.marker.map(at), own) {
                (Some(m), Some(c)) => Some(Cond::Value(when(marked(m, layout.plain), Expr::Truth(Box::new(c)), Value::Bool(false)))),
                (Some(m), None) => Some(marked(m, layout.plain)),
                (None, own) => own,
            };
            let arg = a.arg.clone().map(|e| map_expr_columns(e, &at));
            AggCall { func, arg, filter, distinct: false, result: PART_TYPE, label: format!("{PARTIAL} {}", a.label), ..a.clone() }
        })
        .collect();
    LogicalPlan::Aggregate { groups, collations, aggs, seeds: Vec::new(), input: Box::new(projected) }
}
