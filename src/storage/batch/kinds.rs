//! \[DESIGN\] Vectors of datetimes and of truth values (RULES.md D354), so
//! a scan of a DATE, TIME, TIMESTAMP or BOOLEAN column gives no value made
//! for each row: a datetime's microseconds in UTC as an `i64`, which
//! compare as the datetimes do [ISO/IEC 9075:1992 §4.5.1] (`Datetime::utc`,
//! value/datetime.rs), its displacement beside it only of a type WITH TIME
//! ZONE; a truth value a bit [ISO/IEC 9075-2:2023 §4.6].

use std::sync::OnceLock;

use super::Mask;
use crate::value::datetime::Moment;
use crate::value::{Datetime, Value};

/// \[DESIGN\] Datetimes of one type, each its microseconds in UTC.
#[derive(Debug, Clone)]
pub struct Datetimes {
    /// Each row's microseconds in UTC, a null's 0.
    pub utc: Vec<i64>,
    /// Each row's displacement in minutes, of a type WITH TIME ZONE; none
    /// of one without, whose values carry 0 as the store keeps them.
    pub zones: Option<Vec<i16>>,
    /// Which fields the type has.
    pub moment: Moment,
    /// Whether the type is WITH TIME ZONE.
    pub zoned: bool,
    /// The digits of its seconds' fraction.
    pub precision: u8,
    /// Which rows are null, if any is.
    pub nulls: Option<Mask>,
    /// The values, made the first time they are asked for.
    made: OnceLock<Vec<Value>>,
}

impl Default for Datetimes {
    fn default() -> Datetimes {
        Datetimes::new(Vec::new(), None, (Moment::Date, false, 0), None)
    }
}

impl Datetimes {
    /// Datetimes of `utc`, displaced by `zones` if zoned, of the type
    /// `(moment, zoned, precision)`, null where `nulls` says.
    pub fn new(utc: Vec<i64>, zones: Option<Vec<i16>>, (moment, in_zone, precision): (Moment, bool, u8), nulls: Option<Mask>) -> Datetimes {
        Datetimes { utc, zones, moment, zoned: in_zone, precision, nulls, made: OnceLock::new() }
    }

    /// Whether row `n`'s is null.
    pub fn is_null(&self, n: usize) -> bool {
        self.nulls.as_ref().is_some_and(|nulls| nulls.get(n))
    }

    /// Row `n`'s datetime, none if it is null or past the end.
    pub fn get(&self, n: usize) -> Option<Datetime> {
        let utc = self.utc.get(n).copied().filter(|_| !self.is_null(n))?;
        let zone = self.zones.as_ref().and_then(|z| z.get(n).copied()).unwrap_or(0);
        Some(Datetime { moment: self.moment, utc, zone, zoned: self.zoned, precision: self.precision })
    }

    /// How many rows.
    pub fn len(&self) -> usize {
        self.utc.len()
    }

    /// Whether it has no rows.
    pub fn is_empty(&self) -> bool {
        self.utc.is_empty()
    }

    /// The values, made the first time they are asked for.
    pub fn values(&self) -> &[Value] {
        self.made.get_or_init(|| (0..self.utc.len()).map(|n| self.get(n).map_or(Value::Null, Value::Datetime)).collect())
    }

    /// Its values, owned.
    pub(super) fn into_values(self) -> Vec<Value> {
        self.values();
        self.made.into_inner().unwrap_or_default()
    }

    /// The datetimes of the rows at `picks`, in order, a null for none.
    pub(super) fn gathered(&self, picks: &[Option<usize>]) -> Datetimes {
        let utc = picks.iter().map(|p| p.and_then(|r| self.utc.get(r)).copied().unwrap_or(0)).collect();
        let zones = self.zones.as_ref().map(|z| picks.iter().map(|p| p.and_then(|r| z.get(r)).copied().unwrap_or(0)).collect());
        let nulls = super::gathered::nulls(picks, |r| self.is_null(r));
        Datetimes::new(utc, zones, (self.moment, self.zoned, self.precision), nulls)
    }
}

/// \[DESIGN\] Truth values, True or False a bit each; Unknown is null.
#[derive(Debug, Clone, Default)]
pub struct Bools {
    /// Each row's truth, a null's unset.
    pub bits: Mask,
    /// Which rows are null, if any is.
    pub nulls: Option<Mask>,
    /// The values, made the first time they are asked for.
    made: OnceLock<Vec<Value>>,
}

impl Bools {
    /// The truth values set in `bits`, null where `nulls` says.
    pub fn new(bits: Mask, nulls: Option<Mask>) -> Bools {
        Bools { bits, nulls, made: OnceLock::new() }
    }

    /// Whether row `n`'s is null.
    pub fn is_null(&self, n: usize) -> bool {
        self.nulls.as_ref().is_some_and(|nulls| nulls.get(n))
    }

    /// Row `n`'s truth, none if it is null or past the end.
    pub fn get(&self, n: usize) -> Option<bool> {
        (n < self.bits.len() && !self.is_null(n)).then(|| self.bits.get(n))
    }

    /// How many rows.
    pub fn len(&self) -> usize {
        self.bits.len()
    }

    /// Whether it has no rows.
    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    /// The values, made the first time they are asked for.
    pub fn values(&self) -> &[Value] {
        self.made.get_or_init(|| (0..self.bits.len()).map(|n| self.get(n).map_or(Value::Null, Value::Bool)).collect())
    }

    /// Its values, owned.
    pub(super) fn into_values(self) -> Vec<Value> {
        self.values();
        self.made.into_inner().unwrap_or_default()
    }

    /// The truth values of the rows at `picks`, in order, a null for none.
    pub(super) fn gathered(&self, picks: &[Option<usize>]) -> Bools {
        let mut bits = Mask::none(picks.len());
        for (n, p) in picks.iter().enumerate() {
            if p.is_some_and(|r| self.bits.get(r)) {
                bits.set(n);
            }
        }
        Bools::new(bits, super::gathered::nulls(picks, |r| self.is_null(r)))
    }
}
