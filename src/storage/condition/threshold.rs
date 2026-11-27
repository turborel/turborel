//! \[DESIGN\] (RULES.md D379) A bound a sort of which only the first rows are
//! asked for tells the scan under it, raised as it holds rows. A scan may
//! pass over a group or a part whose values of the sort's first key are all
//! past it, as none of their rows could be among the first, and may leave
//! out a row whose own value is past it (RULES.md D1241). The sort judges
//! each row it is given itself, so a bound read before it was last raised
//! only reads more. A sort run in parallel shares one bound among its
//! morsels' sorts, each raising it once it holds as many rows as are
//! asked, which rows of its own morsel alone already prove. The bound only narrows, a value past it never given
//! again once a row at or before it is held, so a group passed over holds no
//! row the sort would have kept. It is told only of a key whose nulls sort
//! last, as bounds say nothing of a group's nulls, and holds only exact
//! numbers and datetimes, whose comparison is the sort's.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Mutex;

use super::{Condition, Conjunct};
use crate::index::Probe;
use crate::value::{CompOp, Value, compare};

/// \[DESIGN\] (RULES.md D379) The greatest value of a sort's first key a row
/// may hold and still be among the first, or of a descending key the
/// least; none until the sort holds as many rows as are asked.
#[derive(Debug)]
pub struct Threshold {
    descending: bool,
    /// \[DESIGN\] (RULES.md D1241) Of a bound the morsels of a sort run in
    /// parallel share, how many rows the sort asks for: the first keys
    /// its morsels offer are counted to that many. 0 of a bound one sort
    /// raises alone.
    most: usize,
    held: Mutex<Held>,
}

/// What a bound holds: its value, and, of a shared one, the first keys
/// offered that are still among the first so many, the last of them on
/// top.
#[derive(Debug, Default)]
struct Held {
    bound: Option<Value>,
    first: BinaryHeap<Offered>,
}

/// A first key offered, ordered as its sort orders it, so the last of
/// those held is the greatest.
#[derive(Debug)]
struct Offered {
    value: Value,
    descending: bool,
}

impl Ord for Offered {
    fn cmp(&self, other: &Self) -> Ordering {
        // Exact numbers and datetimes of one column compare.
        let order = compare(&self.value, &other.value).unwrap_or(Ordering::Equal);
        if self.descending { order.reverse() } else { order }
    }
}

impl PartialOrd for Offered {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Offered {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Offered {}

impl Threshold {
    /// A bound of a key `descending` or not, not yet set.
    pub fn new(descending: bool) -> Threshold {
        Threshold { descending, most: 0, held: Mutex::new(Held::default()) }
    }

    /// \[DESIGN\] (RULES.md D1241) As `new`, shared by the morsels of a
    /// sort of its first `most` rows, each of which offers the first keys
    /// of the rows it holds (`offer`).
    pub fn shared(descending: bool, most: usize) -> Threshold {
        Threshold { descending, most, held: Mutex::new(Held::default()) }
    }

    /// Narrows the bound to `value`, the first key of the last row held, if
    /// it is narrower: of an exact number or a datetime alone.
    pub fn raise(&self, value: &Value) {
        if !matches!(value, Value::Exact(_) | Value::Datetime(_)) {
            return;
        }
        let Ok(mut held) = crate::sync::lock(&self.held) else { return };
        self.narrow(&mut held, value);
    }

    /// Narrows the bound `held` holds to `value` if it is narrower.
    fn narrow(&self, held: &mut Held, value: &Value) {
        let past = if self.descending { Ordering::Greater } else { Ordering::Less };
        if held.bound.as_ref().is_none_or(|b| compare(value, b) == Some(past)) {
            held.bound = Some(value.clone());
        }
    }

    /// \[DESIGN\] (RULES.md D1241) Of a shared bound, counts `value`, the
    /// first key of a row a morsel's sort holds, each row offered once:
    /// once as many rows as the sort asks for have been offered, the
    /// bound is the last of the first so many, as that many rows of the
    /// sort's input are at or before it, whichever morsels hold them. A
    /// value that is no exact number or datetime is not counted, which
    /// only leaves the bound wider.
    pub fn offer(&self, value: &Value) {
        if self.most == 0 || !matches!(value, Value::Exact(_) | Value::Datetime(_)) {
            return;
        }
        let Ok(mut held) = crate::sync::lock(&self.held) else { return };
        let offered = Offered { value: value.clone(), descending: self.descending };
        if held.first.len() < self.most {
            held.first.push(offered);
        } else if held.first.peek().is_some_and(|last| offered < *last) {
            held.first.pop();
            held.first.push(offered);
        } else {
            return;
        }
        if held.first.len() >= self.most
            && let Some(last) = held.first.peek().map(|last| last.value.clone())
        {
            self.narrow(&mut held, &last);
        }
    }

    /// \[DESIGN\] (RULES.md D1241) The test the bound is of a row now, of
    /// its key's column `column`: a conjunct a scan judges of each row,
    /// that the row's value is at or before the bound as the key is
    /// ordered, so a row equal to it is kept; none while it holds no
    /// value. A row it is not true of is past the greatest row the sort
    /// holds, whatever its other keys, and a null is, as a bound is told
    /// only of a key whose nulls sort last. The bound chooses the kind of
    /// conjunct by the kind of value it holds, here a comparison of exact
    /// numbers or datetimes.
    pub fn row_test(&self, column: usize) -> Option<Conjunct> {
        let op = if self.descending { CompOp::Ge } else { CompOp::Le };
        let value = self.bound()?;
        Some(Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value } }))
    }

    /// Whether it is shared by the morsels of a sort (`shared`).
    pub fn is_shared(&self) -> bool {
        self.most > 0
    }

    /// Whether its key is descending, so the bound is the least value a
    /// row among the first may hold.
    pub fn descending(&self) -> bool {
        self.descending
    }

    /// The bound, if set.
    pub fn bound(&self) -> Option<Value> {
        crate::sync::lock(&self.held).ok().and_then(|held| held.bound.clone())
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

    /// Verifies: [DESIGN] RULES.md D1241: a shared bound holds no value
    /// until as many first keys as the sort asks for are offered, then the
    /// last of the first so many, whichever order they are offered in,
    /// narrowing as one before it is offered and unmoved by one past it;
    /// descending it is the least of the greatest so many; a string is
    /// not counted; and a bound not shared counts none.
    #[test]
    fn a_shared_threshold_is_the_last_of_the_first_offered() {
        let at = |t: &Threshold| t.bound().map(|b| b.to_string());
        let up = Threshold::shared(false, 3);
        for v in [40, 10] {
            up.offer(&n(v));
        }
        up.offer(&Value::Char("a".into()));
        assert_eq!(at(&up), None, "two offered of three");
        up.offer(&n(30));
        assert_eq!(at(&up), Some("40".into()), "the last of the first three");
        up.offer(&n(50));
        assert_eq!(at(&up), Some("40".into()), "one past it");
        up.offer(&n(20));
        assert_eq!(at(&up), Some("30".into()), "one before it");
        up.offer(&n(30));
        assert_eq!(at(&up), Some("30".into()), "one equal to it");
        let down = Threshold::shared(true, 2);
        for v in [5, 9, 7, 1] {
            down.offer(&n(v));
        }
        assert_eq!(at(&down), Some("7".into()), "descending");
        let alone = Threshold::new(false);
        alone.offer(&n(1));
        assert_eq!(at(&alone), None, "a bound not shared counts none");
    }

    /// Verifies: [DESIGN] RULES.md D1241: a bound is no test of a row
    /// until it holds a value; then, of its column, that the row is at or
    /// before it, ascending `<=` and descending `>=`, true of a row equal
    /// to it and of no null.
    #[test]
    fn a_threshold_is_a_test_of_a_row_once_set() {
        for (descending, kept, dropped) in [(false, 3, 8), (true, 8, 3)] {
            let bound = Threshold::new(descending);
            assert!(bound.row_test(4).is_none(), "not set");
            bound.raise(&n(5));
            let test = bound.row_test(4).unwrap();
            assert_eq!(test.columns(), [4]);
            assert!(test.matches_key(&[&n(kept)]) && test.matches_key(&[&n(5)]), "descending {descending}");
            assert!(!test.matches_key(&[&n(dropped)]) && !test.matches_key(&[&Value::Null]), "descending {descending}");
        }
    }
}
