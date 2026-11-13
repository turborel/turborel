//! [ISO/IEC 9075-2:2023 §10.10] How a sort key orders its values, its
//! direction and where its nulls go.

use std::cmp::Ordering;

use super::{NULLS_SORT_HIGH, Value, sort_cmp};

/// [ISO/IEC 9075-2:2023 §10.10] A sort key's `<ordering specification>`
/// and `<null ordering>`, as written or implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SortOrder {
    /// DESC rather than ASC \[GR1c\].
    pub descending: bool,
    /// NULLS FIRST rather than NULLS LAST \[GR1e ii\].
    pub nulls_first: bool,
}

impl SortOrder {
    /// The order of a key `descending` or not, of `written`, the null
    /// ordering written if it was. \[SR3\] Else the null ordering implied
    /// (ID133), which depends on the direction alone: nulls are greater
    /// than every value (`NULLS_SORT_HIGH`), so last ascending and first
    /// descending.
    pub fn of(descending: bool, written: Option<bool>) -> SortOrder {
        SortOrder { descending, nulls_first: written.unwrap_or(descending == NULLS_SORT_HIGH) }
    }

    /// Whether its null ordering is the one implied, which places a null
    /// as `sort_cmp` does, reversed with the direction: as an index or a
    /// table kept in order reads its rows.
    pub fn implied(self) -> bool {
        self.nulls_first == (self.descending == NULLS_SORT_HIGH)
    }

    /// \[GR1e\] The order of two values, `a_null` and `b_null` whether each
    /// is null and `values` their order ascending if neither is: two nulls
    /// are peers \[GR1e ii 1\], a null is before or after a value as NULLS
    /// FIRST or NULLS LAST says, whatever the direction \[GR1e ii 2, 3\],
    /// and two values are in their order, reversed if descending \[GR1c\].
    pub fn placed(self, (a_null, b_null): (bool, bool), values: impl FnOnce() -> Ordering) -> Ordering {
        let null_first = if self.nulls_first { Ordering::Less } else { Ordering::Greater };
        match (a_null, b_null) {
            (true, true) => Ordering::Equal,
            (true, false) => null_first,
            (false, true) => null_first.reverse(),
            (false, false) => {
                let ascending = values();
                if self.descending { ascending.reverse() } else { ascending }
            }
        }
    }

    /// The order of `a` and `b` under it.
    pub fn cmp(self, a: &Value, b: &Value) -> Ordering {
        self.placed((a.is_null(), b.is_null()), || sort_cmp(a, b))
    }

    /// Its `<null ordering>` as written after a key, if not the one
    /// implied: so a key's text tells two orders apart.
    pub fn null_suffix(self) -> &'static str {
        match (self.implied(), self.nulls_first) {
            (true, true | false) => "",
            (false, true) => " NULLS FIRST",
            (false, false) => " NULLS LAST",
        }
    }
}

// Unit tests: where a null goes under each direction and null ordering
// (RULES.md R-TEST-1, R-COV-3).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10 GR1c, GR1e ii] two
    /// nulls are peers, a null is first under NULLS FIRST and last under
    /// NULLS LAST in either direction, and two values are in their order,
    /// reversed if descending: each case of `placed`, MC/DC.
    #[test]
    fn nulls_go_where_the_null_ordering_says() {
        use Ordering::{Equal, Greater, Less};
        let (null, one, two) = (Value::Null, int(1), int(2));
        // (descending, nulls first, a, b, order)
        let cases: [(bool, bool, &Value, &Value, Ordering); 10] = [
            (false, false, &null, &null, Equal),
            (true, true, &null, &null, Equal),
            (false, true, &null, &one, Less),
            (false, false, &null, &one, Greater),
            (true, true, &null, &one, Less),
            (true, false, &null, &one, Greater),
            (false, true, &one, &null, Greater),
            (true, false, &one, &null, Less),
            (false, true, &one, &two, Less),
            (true, true, &one, &two, Greater),
        ];
        for (descending, nulls_first, a, b, want) in cases {
            assert_eq!(SortOrder { descending, nulls_first }.cmp(a, b), want, "{descending} {nulls_first} {a:?} {b:?}");
        }
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10 SR3] the null
    /// ordering implied is NULLS LAST ascending and NULLS FIRST
    /// descending, as `sort_cmp` places nulls, and only it is left
    /// unwritten in a key's text.
    #[test]
    fn the_null_ordering_implied_is_sort_cmps() {
        for descending in [false, true] {
            let implied = SortOrder::of(descending, None);
            assert_eq!(implied.nulls_first, descending);
            assert!(implied.implied());
            assert_eq!(implied.null_suffix(), "");
            let flipped = if descending { sort_cmp(&int(1), &Value::Null) } else { sort_cmp(&Value::Null, &int(1)) };
            assert_eq!(implied.cmp(&Value::Null, &int(1)), flipped);
        }
        assert_eq!(SortOrder::of(false, Some(true)).null_suffix(), " NULLS FIRST");
        assert_eq!(SortOrder::of(true, Some(false)).null_suffix(), " NULLS LAST");
        assert!(!SortOrder::of(true, Some(false)).implied());
    }
}
