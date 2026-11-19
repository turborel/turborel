//! Spatial window candidates preserve D15 errors. The indexed column
//! enforces an SRID; every other conjunct is non-raising. The index must
//! fall back to all entries when a later prepared argument changes
//! reference, and the full SQL comparison remains a residual. A relate
//! pattern is a constant that implies meeting; a distance unit is a
//! constant that converts for the column's SRID, or no window is planned,
//! so the scan raises its error (D135).

use super::{Candidate, predicate};
use crate::catalog::{IndexDesc, SpatialCatalog};
use crate::geometry::{Geometry, fixed_srid, unit_scale};
use crate::index::{Predicate, Probe};
use crate::logical::{Cond, Slot};
use crate::physical::spatial::{Beside, recognized};
use crate::physical::{Place, ProbeSlot};
use crate::spatial::window;
use crate::value::{DataType, Value};

/// What window candidates are planned against: the index and its key
/// types, the statement's values, and the reference systems units name.
pub(super) struct Against<'a> {
    pub(super) ix: &'a IndexDesc,
    pub(super) types: &'a [DataType],
    pub(super) values: Option<&'a [Value]>,
    pub(super) systems: &'a SpatialCatalog,
}

pub(super) fn candidates(conds: &[Cond], against: &Against<'_>, all: f64) -> Vec<Candidate> {
    let [data_type] = against.types else { return Vec::new() };
    let Some(srid) = fixed_srid(data_type).filter(|_| !against.ix.is_collated()) else { return Vec::new() };
    conds
        .iter()
        .enumerate()
        .filter_map(|(at, cond)| {
            if conds.iter().enumerate().any(|(other, c)| other != at && c.can_raise()) {
                return None;
            }
            let found = recognized(cond)?;
            let (column, argument, other) = found.split()?;
            if against.ix.columns.as_slice() != [column] {
                return None;
            }
            let (geometry, slot) = predicate::constant(other, against.values)?;
            let valid = geometry.is_null() || Geometry::from_value(&geometry).is_ok();
            if !valid {
                return None;
            }
            let (value, slots) = probed((geometry, slot), &found.beside, (srid, against))?;
            let predicate = Box::new(Predicate { function: found.function.clone(), column: 0, argument: Some(argument) });
            let probe = Probe::Predicate { predicate, value };
            Some(Candidate { slots, ..Candidate::one(probe, (vec![at], 0, all, true)) })
        })
        .collect()
}

/// The probe's value of the geometry and what is read beside it, and the
/// places of its `?`s: the geometry's alone, or the first field of a ROW
/// and a distance limit's the second.
fn probed(
    (geometry, slot): (Value, Option<Slot>),
    beside: &Beside<'_>,
    (srid, against): (i32, &Against<'_>),
) -> Option<(Value, Vec<ProbeSlot>)> {
    let field = |given: Option<Slot>, n: usize| given.map(|at| ProbeSlot { place: Place::PredicateField(n), slot: at });
    match beside {
        Beside::Nothing => {
            let slots = slot.into_iter().map(|slot| ProbeSlot { place: Place::PredicateValue, slot }).collect();
            Some((geometry, slots))
        }
        Beside::Pattern(pattern) => Some((window::related(geometry, (*pattern).clone()), field(slot, 0).into_iter().collect())),
        Beside::Within(limit, unit) => {
            let (limit, limit_slot) = predicate::constant(limit, against.values)?;
            let scale = match unit {
                Some(unit) => unit_scale(srid, Some(against.systems), unit).ok()?,
                None => 1.0_f64,
            };
            let slots = field(slot, 0).into_iter().chain(field(limit_slot, 1)).collect();
            Some((window::within(geometry, limit, scale), slots))
        }
    }
}
