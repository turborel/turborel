//! Spatial intersection candidates preserve D15 errors. The indexed
//! column enforces an SRID; every other conjunct is non-raising. The
//! index must fall back to all entries when a later prepared argument
//! changes reference, and the full SQL comparison remains a residual.

use super::{Candidate, predicate};
use crate::catalog::IndexDesc;
use crate::geometry::{Geometry, fixed_srid};
use crate::index::{Predicate, Probe};
use crate::logical::Cond;
use crate::physical::spatial::intersection;
use crate::physical::{Place, ProbeSlot};
use crate::value::{DataType, Value};

pub(super) fn candidates(conds: &[Cond], ix: &IndexDesc, types: &[DataType], values: Option<&[Value]>, all: f64) -> Vec<Candidate> {
    let [data_type] = types else { return Vec::new() };
    if ix.is_collated() || fixed_srid(data_type).is_none() {
        return Vec::new();
    }
    conds
        .iter()
        .enumerate()
        .filter_map(|(at, cond)| {
            if conds.iter().enumerate().any(|(other, c)| other != at && c.can_raise()) {
                return None;
            }
            let (function, args) = intersection(cond)?;
            let (column, argument, other) = predicate::operands(args)?;
            if ix.columns.as_slice() != [column] {
                return None;
            }
            let (value, slot) = predicate::constant(other, values)?;
            let valid = value.is_null() || Geometry::from_value(&value).is_ok();
            if !valid {
                return None;
            }
            let predicate = Box::new(Predicate { function: function.clone(), column: 0, argument });
            let probe = Probe::Predicate { predicate, value };
            let slots = slot.into_iter().map(|slot| ProbeSlot { place: Place::PredicateValue, slot }).collect();
            Some(Candidate { slots, ..Candidate::one(probe, (vec![at], 0, all, true)) })
        })
        .collect()
}
