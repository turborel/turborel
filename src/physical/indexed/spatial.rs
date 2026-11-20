//! Geometry joins through envelope candidates (D15, D85, D135). The
//! indexed column enforces an SRID, other conjuncts and the inner filter
//! cannot raise, and the full ON condition checks every visible pair. An
//! outer geometry of another reference makes its probe read every entry,
//! so the condition raises at the first pair, as the product would; only
//! two columns of one fixed SRID prove the join error-free. Outer joins
//! keep their unmatched rows (`spatial/outer.rs`).

use super::probed::{Choice, Preserve, planned};
use super::{Read, read_table};
use crate::catalog::IndexDesc;
use crate::functions::ScalarRef;
use crate::geometry::{SpatialOperation, fixed_srid, unit_scale};
use crate::index::Predicate;
use crate::logical::{Cond, Expr, LogicalPlan, SortKey, split_and};
use crate::physical::join::Flip;
use crate::physical::shown::Shown;
use crate::physical::spatial::{Beside, Recognized, recognized};
use crate::physical::{Built, Fields, JoinProbe, PhysicalPlanner, Plan, Supplied, of_left};
use crate::storage::cost_of;
use crate::value::Value;

mod outer;
mod paired;

struct Binding {
    function: ScalarRef,
    outer: usize,
    inner: usize,
    argument: usize,
    beside: Besides,
    /// A relationship true only of meeting operands, which, of two
    /// columns of one SRID, raises nothing (D85).
    windowed: bool,
}

/// What a window reads beside the outer geometry, owned (`Beside`).
enum Besides {
    Nothing,
    Pattern(Value),
    Within(Supplied, Option<String>),
}

impl PhysicalPlanner<'_> {
    pub(in crate::physical) fn spatial_join(
        &self,
        cond: &Cond,
        (left, right): (&LogicalPlan, &LogicalPlan),
        want: &[SortKey],
    ) -> Option<Built> {
        let flip = Flip { width: left.width(), right_width: right.width() };
        let reversed = flip.cond(cond.clone());
        let normal = self.geometry_probe((left, right), cond, Preserve::Inner);
        let swapped = self.geometry_probe((right, left), &reversed, Preserve::Inner);
        let swap = swapped.as_ref().is_some_and(|s| normal.as_ref().is_none_or(|n| s.total < n.total));
        let names = self.shown.then(|| left.names().into_iter().chain(right.names()).collect::<Vec<_>>());
        let shown = Shown::of(names.as_deref());
        if swap {
            let mut choice = swapped?;
            choice.spatial &= want.is_empty();
            choice.paired = choice.paired.filter(|_| want.is_empty());
            return Some(Built::unordered(self.swapped(choice, right, (reversed, flip, shown))));
        }
        let mut choice = normal?;
        choice.spatial &= want.is_empty();
        choice.paired = choice.paired.filter(|_| want.is_empty());
        let wanted = of_left(want, left.width());
        let outer = self.build_node(left, wanted);
        let ordered = !wanted.is_empty() && outer.ordered;
        let plan = planned(choice, outer.plan, (cond.clone(), left.width()), shown);
        Some(Built { plan, ordered })
    }

    /// `choice`, a probe of the left operand's table by the rows of
    /// `right`, under a projection putting the left's columns first again.
    fn swapped(&self, choice: Choice, right: &LogicalPlan, (reversed, flip, shown): (Cond, Flip, Shown<'_>)) -> Plan {
        let outer = self.build_node(right, &[]).plan;
        let flipped_names = flip.names(shown);
        let flipped = Shown::of(flipped_names.as_deref());
        let plan = planned(choice, outer, (reversed, right.width()), flipped);
        flip.restored(plan, flipped)
    }

    /// The cheapest probe of `inner`'s table by `outer`'s rows on `cond`,
    /// keeping the unmatched rows `preserve` says.
    fn geometry_probe(&self, (outer, inner): (&LogicalPlan, &LogicalPlan), cond: &Cond, preserve: Preserve) -> Option<Choice> {
        let binding = crossed(cond, outer.width())?;
        let read = read_table(inner)?;
        let srid = fixed_srid(&self.column_type(inner, binding.inner)?)?;
        let same = self.column_type(outer, binding.outer).as_ref().and_then(fixed_srid) == Some(srid);
        let proved = same && binding.windowed && preserve == Preserve::Inner;
        let supplied = self.supplied(&binding.beside, srid)?;
        let beside = (!supplied.is_empty()).then(|| Box::new(Fields(supplied)));
        let outer_rows = self.rows_of(outer);
        let rows = cost_of(self.db.storage_of(read.table).ok()?.row_count());
        let mut best: Option<Choice> = None;
        // [EXTENSION] a global index is probed by equalities alone (RULES.md D157).
        for ix in self.db.catalog.indexes(read.table).into_iter().filter(|ix| !crate::catalog::is_global_index(&ix.name)) {
            let Some(predicate) = self.geometry_key(&read, ix, &binding) else { continue };
            let probe = JoinProbe::Predicate { predicate, outer: binding.outer, beside: beside.clone() };
            // Until geometry statistics exist, use the two-dimensional
            // range estimate. It affects cost only, never the residual.
            let range = self.weights().range_selectivity;
            let mut choice = self.join_cost(&read, ix, probe, (outer_rows, range * range))?;
            (choice.spatial, choice.preserve) = (proved, preserve);
            if best.as_ref().is_none_or(|b| choice.total < b.total) {
                best = Some(choice);
            }
        }
        let mut best = best?;
        best.paired = self.paired(outer, &binding, &best, same);
        // Probes dearer than the product are no plan, but a traversal,
        // which compares each entry once, costs less than the product.
        (best.paired.is_some() || best.total < outer_rows * rows).then_some(best)
    }

    /// The values a window reads beside the geometry: a unit's scale, of
    /// the indexed column's SRID, only if it converts, as the unit form
    /// would raise of every pair otherwise.
    fn supplied(&self, beside: &Besides, srid: i32) -> Option<Box<[Supplied]>> {
        match beside {
            Besides::Nothing => Some(Box::new([])),
            Besides::Pattern(pattern) => Some(Box::new([Supplied::Constant(pattern.clone())])),
            Besides::Within(limit, unit) => {
                let scale = match unit {
                    Some(unit) => unit_scale(srid, Some(self.db.catalog.spatial()), unit).ok()?,
                    None => 1.0_f64,
                };
                Some(Box::new([limit.clone(), Supplied::Constant(Value::Approx(scale))]))
            }
        }
    }

    fn geometry_key(&self, read: &Read<'_>, ix: &IndexDesc, binding: &Binding) -> Option<Box<Predicate>> {
        if self.db.building.contains(&ix.name) || ix.is_collated() {
            return None;
        }
        let column = match &read.through {
            Some((columns, _)) => *columns.get(binding.inner)?,
            None => binding.inner,
        };
        if ix.columns.as_slice() != [column] {
            return None;
        }
        self.db.index_of(&ix.name).ok()?;
        let kind = self.db.methods.get(&ix.method)?;
        let types = ix.key_types(self.db.catalog.table(read.table)?)?;
        let predicate = Predicate { function: binding.function.clone(), column: 0, argument: Some(binding.argument) };
        kind.supports_predicate(&predicate, &types).then(|| Box::new(predicate))
    }
}

/// The one windowed spatial conjunct of `cond` that crosses its operands,
/// the outer `width` columns wide, if every other conjunct cannot raise.
fn crossed(cond: &Cond, width: usize) -> Option<Binding> {
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let mut found = None;
    for c in &conjuncts {
        if let Some(spatial) = recognized(c) {
            if found.is_some() {
                return None;
            }
            found = Some(bound(&spatial, width)?);
        } else if c.can_raise() {
            return None;
        }
    }
    found
}

fn bound(spatial: &Recognized<'_>, width: usize) -> Option<Binding> {
    let [Expr::Column(a), Expr::Column(b)] = spatial.operands else { return None };
    let (outer, inner, argument) = if *a < width && *b >= width {
        (*a, b.saturating_sub(width), 1)
    } else if *b < width && *a >= width {
        (*b, a.saturating_sub(width), 0)
    } else {
        return None;
    };
    let beside = match &spatial.beside {
        Beside::Nothing => Besides::Nothing,
        Beside::Pattern(pattern) => Besides::Pattern((*pattern).clone()),
        Beside::Within(limit, unit) => Besides::Within(limit_of(limit, width)?, unit.map(str::to_owned)),
    };
    let windowed = spatial.function.0.spatial_operation().is_some_and(SpatialOperation::windowed);
    Some(Binding { function: spatial.function.clone(), outer, inner, argument, beside, windowed })
}

/// A distance's limit, a constant or a column of the outer operand.
fn limit_of(limit: &Expr, width: usize) -> Option<Supplied> {
    if let Expr::Const(value) = limit {
        return Some(Supplied::Constant(value.clone()));
    }
    let Expr::Column(column) = limit else { return None };
    (*column < width).then_some(Supplied::Outer(*column))
}
