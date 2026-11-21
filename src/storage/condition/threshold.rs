//! \[DESIGN\] (RULES.md D379) A bound a sort of which only the first rows are
//! asked for tells the scan under it, raised as it holds rows. A scan may
//! pass over a group or a part whose values of the sort's first key are all
//! past it, as none of their rows could be among the first. The sort judges
//! each row it is given itself, so a bound read before it was last raised
//! only reads more. The bound only narrows, a value past it never given
//! again once a row at or before it is held, so a group passed over holds no
//! row the sort would have kept. It is told only of a key whose nulls sort
//! last, as bounds say nothing of a group's nulls, and holds only exact
//! numbers and datetimes, whose comparison is the sort's.

use std::cmp::Ordering;
use std::sync::Mutex;

use crate::value::{Value, compare};

/// \[DESIGN\] (RULES.md D379) The greatest value of a sort's first key a row
/// may hold and still be among the first, or of a descending key the
/// least; none until the sort holds as many rows as are asked.
#[derive(Debug)]
pub struct Threshold {
    descending: bool,
    bound: Mutex<Option<Value>>,
}

impl Threshold {
    /// A bound of a key `descending` or not, not yet set.
    pub fn new(descending: bool) -> Threshold {
        Threshold { descending, bound: Mutex::new(None) }
    }

    /// Narrows the bound to `value`, the first key of the last row held, if
    /// it is narrower: of an exact number or a datetime alone.
    pub fn raise(&self, value: &Value) {
        if !matches!(value, Value::Exact(_) | Value::Datetime(_)) {
            return;
        }
        let Ok(mut bound) = crate::sync::lock(&self.bound) else { return };
        let past = if self.descending { Ordering::Greater } else { Ordering::Less };
        if bound.as_ref().is_none_or(|b| compare(value, b) == Some(past)) {
            *bound = Some(value.clone());
        }
    }

    /// The bound, if set.
    pub fn bound(&self) -> Option<Value> {
        crate::sync::lock(&self.bound).ok().and_then(|b| b.clone())
    }

    /// Whether a row whose value of the key is within `range`, its least
    /// and greatest not null, may be at or before the bound: true if either
    /// is unknown, or the values do not compare.
    pub fn may_match(&self, range: Option<&(Value, Value)>) -> bool {
        let (Some(bound), Some((least, greatest))) = (self.bound(), range) else { return true };
        if self.descending { compare(greatest, &bound) != Some(Ordering::Less) } else { compare(least, &bound) != Some(Ordering::Greater) }
    }
}

// Unit tests: the bound narrows only, and a range is passed over only past
// it (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn n(v: i128) -> Value {
        Value::Exact(Decimal::new(v, 0))
    }

    /// Verifies: [DESIGN] RULES.md D379: a bound not set passes over
    /// nothing; set, it narrows only, ascending to lesser values and
    /// descending to greater; a range is passed over only if every value of
    /// it is past the bound, a value equal to it kept; a string never sets
    /// it.
    #[test]
    fn a_threshold_narrows_and_passes_over_ranges_past_it() {
        let up = Threshold::new(false);
        assert!(up.may_match(Some(&(n(100), n(200)))), "not set");
        up.raise(&Value::Char("a".into()));
        assert!(up.bound().is_none(), "a string sets none");
        up.raise(&n(50));
        up.raise(&n(70));
        assert_eq!(up.bound().and_then(|b| compare(&b, &n(50))), Some(Ordering::Equal), "narrows only");
        assert!(!up.may_match(Some(&(n(51), n(90)))), "past it");
        assert!(up.may_match(Some(&(n(50), n(90)))), "equal kept");
        assert!(up.may_match(None), "no values known");
        let down = Threshold::new(true);
        down.raise(&n(50));
        down.raise(&n(30));
        assert_eq!(down.bound().and_then(|b| compare(&b, &n(50))), Some(Ordering::Equal));
        assert!(!down.may_match(Some(&(n(10), n(49)))));
        assert!(down.may_match(Some(&(n(10), n(50)))));
    }
}
