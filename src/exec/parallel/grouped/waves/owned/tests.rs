//! Unit tests of the morsels that own groups (RULES.md D1121): which are
//! clean by their bounds.

use super::*;
use crate::value::Decimal;

/// Bounds of exact numbers, `lo` to `hi`.
fn bounds(lo: i64, hi: i64) -> (Value, Value) {
    (Value::Exact(Decimal::new(lo.into(), 0)), Value::Exact(Decimal::new(hi.into(), 0)))
}

/// Which of `bounds` are clean.
fn clean(bounds: Vec<Option<(Value, Value)>>) -> Vec<bool> {
    cleaned(bounds).unwrap().iter().map(Option::is_some).collect()
}

/// Verifies: [DESIGN] (RULES.md D1121) a morsel is clean if every other
/// morsel's bounds lie apart from its own or meet them at an end alone,
/// whatever order the morsels come in: one that another before it in
/// order ends past, one the next begins within, both, or neither, of
/// bounds one inside another, of one least value, and of one value
/// alone. MC/DC of `!ends_past && !next_begins`: each false alone, and
/// both true.
#[test]
fn a_morsel_is_clean_if_no_other_bounds_meet_its_own_beyond_an_end() {
    assert_eq!(clean(vec![Some(bounds(7, 9)), Some(bounds(1, 3)), Some(bounds(4, 6))]), [true, true, true], "apart, out of order");
    assert_eq!(
        clean(vec![Some(bounds(1, 3)), Some(bounds(3, 5)), Some(bounds(5, 5)), Some(bounds(5, 8))]),
        [true; 4],
        "meeting at ends alone"
    );
    // [1, 5] the next begins within; [3, 7] the one before ends past.
    assert_eq!(clean(vec![Some(bounds(1, 5)), Some(bounds(3, 7)), Some(bounds(8, 9))]), [false, false, true]);
    // [2, 3] lies within [1, 10], which is not the one just before [11, 12].
    assert_eq!(clean(vec![Some(bounds(1, 10)), Some(bounds(2, 3)), Some(bounds(11, 12))]), [false, false, true]);
    // [4, 6] both: [1, 5] ends past it and [5, 9] begins within it.
    assert_eq!(clean(vec![Some(bounds(1, 5)), Some(bounds(4, 6)), Some(bounds(5, 9))]), [false, false, false]);
    assert_eq!(clean(vec![Some(bounds(3, 5)), Some(bounds(3, 9))]), [false, false], "one least value");
    assert_eq!(clean(vec![Some(bounds(3, 3)), Some(bounds(3, 9))]), [true, true], "a value alone meets at an end");
}

/// Verifies: [DESIGN] (RULES.md D1121) a morsel of no value but nulls is
/// in no other's way and owns nothing; and of bounds two of which do not
/// compare, no morsel owns.
#[test]
fn a_morsel_of_nulls_owns_nothing_and_bounds_that_do_not_compare_none() {
    assert_eq!(clean(vec![Some(bounds(1, 3)), None, Some(bounds(4, 6))]), [true, false, true]);
    let text = Some((Value::Char("a".into()), Value::Char("b".into())));
    assert!(cleaned(vec![Some(bounds(1, 3)), text]).is_none());
}
