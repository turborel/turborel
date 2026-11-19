//! \[DESIGN\] Conservative rectangle candidates for ordinary SQL
//! comparisons. Exact keys are checked with Probe::answers afterward.
//! Unplaced constants leave a coordinate unbounded, never rounded inward.

use crate::index::{ColumnComparison, Key, Probe};
use crate::spatial::{Bounds, DIMENSIONS};
use crate::value::{CompOp, Value};

pub(super) fn key(bounds: Bounds, width: usize) -> Key {
    let coordinates = bounds.lower().into_iter().chain(bounds.upper());
    Key(coordinates.take(width).map(Value::Approx).collect())
}

/// Check candidate coordinates on the stack; only matches allocate keys.
pub(super) fn answers(probe: &Probe, bounds: Bounds, width: usize) -> bool {
    let [lx, ly] = bounds.lower();
    let [hx, hy] = bounds.upper();
    let values = [lx, ly, hx, hy].map(Value::Approx);
    probe.answers(values.iter().take(width))
}

/// None is an empty result. Four-column keys denote rectangles; crossed
/// necessary half-planes can describe a rectangle spanning both bounds,
/// so use their hull rather than incorrectly declaring them impossible.
pub(super) fn window(probe: &Probe, width: usize) -> Option<Bounds> {
    let mut low = [-f64::MAX; DIMENSIONS];
    let mut high = [f64::MAX; DIMENSIONS];
    for c in comparisons(probe) {
        if c.column >= width || c.value.is_null() {
            return None;
        }
        let Value::Approx(at) = c.value else { continue };
        if !at.is_finite() {
            continue;
        }
        let axis = c.column.checked_rem(DIMENSIONS)?;
        let (lo, hi) = (low.get_mut(axis)?, high.get_mut(axis)?);
        match c.op {
            CompOp::Eq => {
                *lo = lo.max(at);
                *hi = hi.min(at);
            }
            CompOp::Gt | CompOp::Ge => *lo = lo.max(at),
            CompOp::Lt | CompOp::Le => *hi = hi.min(at),
            CompOp::Ne => (),
        }
    }
    if width > DIMENSIONS {
        for (lo, hi) in low.iter_mut().zip(&mut high) {
            if *lo > *hi {
                std::mem::swap(lo, hi);
            }
        }
    }
    Bounds::new(low, high).ok()
}

fn comparisons(probe: &Probe) -> Vec<ColumnComparison> {
    let column = |column, op, value: &Value| ColumnComparison { column, op, value: value.clone() };
    let prefix = |equal: &[Value]| equal.iter().enumerate().map(|(at, v)| column(at, CompOp::Eq, v)).collect::<Vec<_>>();
    match probe {
        Probe::All | Probe::Predicate { .. } | Probe::Xml(_) => Vec::new(),
        Probe::Compare { op, value } => vec![column(0, *op, value)],
        Probe::Columns(each) => each.clone(),
        Probe::Prefix { equal, then } => {
            let mut out = prefix(equal);
            if let Some((op, value)) = then {
                out.push(column(equal.len(), *op, value));
            }
            out
        }
        Probe::Range { equal, low, high } => {
            let mut out = prefix(equal);
            out.push(column(equal.len(), low.0, &low.1));
            out.push(column(equal.len(), high.0, &high.1));
            out
        }
    }
}
