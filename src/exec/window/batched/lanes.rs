//! \[DESIGN\] RULES.md D560: the vectors a window over batches computes
//! of and into. A `Lane` is one argument's values over the rows held, its
//! batches' vectors set one after another: integers of one scale and
//! approximate numbers kept as numbers, so a set function reads them with
//! no value made; any other kind, or kinds not alike, as values. A
//! `Built` is a call's values of the rows held, made as numbers while
//! every one is an exact number of one scale within 64 bits, else values,
//! then cut into a vector of each batch given.

use crate::storage::mask::mask_of;
use crate::storage::{Column, Ints, Mask};
use crate::value::{Decimal, Value};

/// One argument's values over the rows held.
pub(in crate::exec::window) enum Lane {
    /// Exact numbers of one scale: their units, a null's 0, and which are
    /// null.
    Ints { units: Vec<i64>, scale: u32, nulls: Vec<bool> },
    /// Approximate numbers, a null's 0, and which are null.
    Floats { numbers: Vec<f64>, nulls: Vec<bool> },
    /// Any other values.
    Values(Vec<Value>),
}

impl Lane {
    /// The vectors `columns` set one after another.
    pub(in crate::exec::window) fn of(columns: &[&Column]) -> Lane {
        if let Some(lane) = ints_of(columns) {
            return lane;
        }
        if let Some(lane) = floats_of(columns) {
            return lane;
        }
        Lane::Values(columns.iter().flat_map(|c| c.values().iter().cloned()).collect())
    }

    /// How many values it holds.
    pub(in crate::exec::window) fn len(&self) -> usize {
        match self {
            Lane::Ints { units, .. } => units.len(),
            Lane::Floats { numbers, .. } => numbers.len(),
            Lane::Values(values) => values.len(),
        }
    }

    /// Its values from `from` up to `to`, a lane of their own.
    pub(in crate::exec::window) fn slice(&self, (from, to): (usize, usize)) -> Lane {
        let cut = |nulls: &[bool]| nulls.get(from..to).unwrap_or_default().to_vec();
        match self {
            Lane::Ints { units, scale, nulls } => {
                Lane::Ints { units: units.get(from..to).unwrap_or_default().to_vec(), scale: *scale, nulls: cut(nulls) }
            }
            Lane::Floats { numbers, nulls } => {
                Lane::Floats { numbers: numbers.get(from..to).unwrap_or_default().to_vec(), nulls: cut(nulls) }
            }
            Lane::Values(values) => Lane::Values(values.get(from..to).unwrap_or_default().to_vec()),
        }
    }

    /// Whether the `n`th is null: so of one past the end.
    pub(in crate::exec::window) fn is_null(&self, n: usize) -> bool {
        match self {
            Lane::Ints { nulls, .. } | Lane::Floats { nulls, .. } => nulls.get(n).is_none_or(|&b| b),
            Lane::Values(values) => values.get(n).is_none_or(Value::is_null),
        }
    }

    /// The `n`th, as a value; null past the end.
    pub(in crate::exec::window) fn value(&self, n: usize) -> Value {
        if self.is_null(n) {
            return Value::Null;
        }
        match self {
            Lane::Ints { units, scale, .. } => units.get(n).map_or(Value::Null, |&u| Value::Exact(Decimal::new(i128::from(u), *scale))),
            Lane::Floats { numbers, .. } => numbers.get(n).map_or(Value::Null, |&f| Value::Approx(f)),
            Lane::Values(values) => values.get(n).cloned().unwrap_or(Value::Null),
        }
    }
}

/// The units and nulls of a vector of exact numbers.
type Units<'a> = (&'a [i64], Option<&'a Mask>);

/// Of `columns` all exact numbers of one scale, their lane.
fn ints_of(columns: &[&Column]) -> Option<Lane> {
    let mut scale = None;
    let mut parts: Vec<Units<'_>> = Vec::with_capacity(columns.len());
    for column in columns {
        let Column::Ints(ints) = column else { return None };
        if scale.is_some_and(|s| s != ints.scale) {
            return None;
        }
        scale = Some(ints.scale);
        parts.push((&ints.units, ints.nulls.as_ref()));
    }
    let (mut units, mut nulls) = (Vec::new(), Vec::new());
    for (u, n) in parts {
        match n {
            None => units.extend_from_slice(u),
            // A null's place holds whatever its vector's maker left there
            // (RULES.md D1289), and a sum over a frame adds every place.
            Some(mask) => units.extend(u.iter().enumerate().map(|(r, &unit)| if mask.get(r) { 0_i64 } else { unit })),
        }
        nulls.extend((0..u.len()).map(|r| n.is_some_and(|m| m.get(r))));
    }
    Some(Lane::Ints { units, scale: scale.unwrap_or_default(), nulls })
}

/// Of `columns` all approximate numbers, their lane.
fn floats_of(columns: &[&Column]) -> Option<Lane> {
    let (mut numbers, mut nulls) = (Vec::new(), Vec::new());
    for column in columns {
        let Column::Floats(floats) = column else { return None };
        numbers.extend(floats.numbers.iter().enumerate().map(|(r, &f)| if floats.is_null(r) { 0.0_f64 } else { f }));
        nulls.extend((0..floats.numbers.len()).map(|r| floats.is_null(r)));
    }
    Some(Lane::Floats { numbers, nulls })
}

/// A call's values of the rows held, made in order.
pub(in crate::exec::window) enum Built {
    /// Exact numbers of one scale, each within 64 bits, and which are
    /// null; of no value yet, scale none.
    Ints { units: Vec<i64>, scale: Option<u32>, nulls: Vec<bool> },
    /// Any values.
    Values(Vec<Value>),
}

impl Default for Built {
    fn default() -> Built {
        Built::Ints { units: Vec::new(), scale: None, nulls: Vec::new() }
    }
}

impl Built {
    /// Adds `value`, as numbers while it is one of them.
    pub(in crate::exec::window) fn push(&mut self, value: Value) {
        if self.pushed_int(&value) {
            return;
        }
        if let Built::Ints { .. } = self {
            *self = Built::Values(self.values());
        }
        if let Built::Values(values) = self {
            values.push(value);
        }
    }

    /// Whether `value` was added as a number: null, or an exact number of
    /// the scale of those before it, within 64 bits.
    fn pushed_int(&mut self, value: &Value) -> bool {
        let Built::Ints { units, scale, nulls } = self else { return false };
        if value.is_null() {
            units.push(0);
            nulls.push(true);
            return true;
        }
        if let Value::Exact(d) = value
            && let Ok(u) = i64::try_from(d.units)
            && scale.is_none_or(|s| s == d.scale)
        {
            *scale = Some(d.scale);
            units.push(u);
            nulls.push(false);
            return true;
        }
        false
    }

    /// Adds the exact number `units` of scale 0, a count or a rank.
    pub(in crate::exec::window) fn push_count(&mut self, units: usize) {
        self.push(Value::Exact(Decimal::new(i128::try_from(units).unwrap_or(i128::MAX), 0)));
    }

    /// Its values so far.
    fn values(&self) -> Vec<Value> {
        match self {
            Built::Ints { units, scale, nulls } => units
                .iter()
                .zip(nulls)
                .map(|(&u, &null)| if null { Value::Null } else { Value::Exact(Decimal::new(i128::from(u), scale.unwrap_or_default())) })
                .collect(),
            Built::Values(values) => values.clone(),
        }
    }

    /// The vector of its values from `from` up to `to`.
    pub(in crate::exec::window) fn column(&self, (from, to): (usize, usize)) -> Column {
        match self {
            Built::Ints { units, scale, nulls } => {
                let cut = units.get(from..to).unwrap_or_default().to_vec();
                let nulls = mask_of(nulls.get(from..to).unwrap_or_default());
                Column::Ints(Ints::new(cut, scale.unwrap_or_default(), nulls))
            }
            Built::Values(values) => Column::Values(values.get(from..to).unwrap_or_default().to_vec()),
        }
    }
}

// Unit tests: a lane's places of nulls hold no number (RULES.md R-TEST-1,
// D1289).
#[cfg(test)]
mod tests;
