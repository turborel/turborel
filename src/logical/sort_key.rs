//! [X3.135-1986 §8.3 GR3b] A sort key: a column of a node's output, its
//! direction, where its nulls go, and whether its character strings are
//! ordered by their octets (RULES.md D1185).

use std::cmp::Ordering;

use crate::value::{SortOrder, Value, sort_cmp};

/// [X3.135-1986 §8.3 GR3b] a sort column of the node's output, and its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    /// The column's position in the node's output.
    pub column: usize,
    /// DESC rather than ASC.
    pub descending: bool,
    /// [ISO/IEC 9075-2:2023 §10.10 GR1e ii] NULLS FIRST rather than NULLS
    /// LAST, as written or implied (`SortOrder::of`).
    pub nulls_first: bool,
    /// \[DESIGN\] (RULES.md D1185) Whether the column's character strings
    /// are ordered by their octets, a prefix the less, as a collation of
    /// NO PAD whose sort key is the text itself orders them [ISO/IEC
    /// 9075-2:2023 §4.3.2], in place of the padded order of the default.
    /// The physical plan's pruning alone sets it, of a sort that read a
    /// key made of the column; no logical plan's key has it.
    pub octets: bool,
}

impl SortKey {
    /// Column `column` in the order `order`.
    pub fn new(column: usize, order: SortOrder) -> SortKey {
        SortKey { column, descending: order.descending, nulls_first: order.nulls_first, octets: false }
    }

    /// Column `column`, ascending, nulls placed as implied.
    pub fn ascending(column: usize) -> SortKey {
        SortKey::new(column, SortOrder::of(false, None))
    }

    /// How it orders its column's values.
    pub fn order(&self) -> SortOrder {
        SortOrder { descending: self.descending, nulls_first: self.nulls_first }
    }

    /// [ISO/IEC 9075-2:2023 §10.10 GR1] The order of `a` and `b` under it:
    /// nulls placed by its null ordering, values in their order, reversed
    /// if descending; two character strings of a key by octets as their
    /// octets order (RULES.md D1185).
    pub fn cmp(&self, a: &Value, b: &Value) -> Ordering {
        self.order().placed((a.is_null(), b.is_null()), || {
            if self.octets
                && let (Value::Char(x), Value::Char(y)) = (a, b)
            {
                return x.as_bytes().cmp(y.as_bytes());
            }
            sort_cmp(a, b)
        })
    }
}

// Unit tests: a key by octets against the padded order (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §10.10 GR1]
    /// RULES.md D1185: a key by octets orders two character strings by
    /// their octets, so a string and itself with a trailing space are
    /// apart, the shorter the less, and a string followed by a character
    /// below a space is the greater, where the padded order has the first
    /// pair equal and the second the other way round; nulls are placed
    /// and DESC reverses as of any key; values that are not both
    /// character strings compare as they did.
    ///
    /// MC/DC of `self.octets && both character strings`: a key by octets
    /// of two strings (both true), a key not by octets of the same strings
    /// (the first false), a key by octets of two numbers (the second
    /// false).
    #[test]
    fn a_key_by_octets_orders_strings_unpadded() {
        let text = |s: &str| Value::Char(s.into());
        let padded = SortKey::ascending(0);
        let octets = SortKey { octets: true, ..padded };
        assert_eq!(padded.cmp(&text("a"), &text("a ")), Ordering::Equal);
        assert_eq!(octets.cmp(&text("a"), &text("a ")), Ordering::Less);
        assert_eq!(padded.cmp(&text("a\t"), &text("a")), Ordering::Less);
        assert_eq!(octets.cmp(&text("a\t"), &text("a")), Ordering::Greater);
        let down = SortKey { descending: true, ..octets };
        assert_eq!(down.cmp(&text("a"), &text("a ")), Ordering::Greater);
        assert_eq!(octets.cmp(&Value::Null, &text("a")), Ordering::Greater, "nulls last, as implied ascending");
        let n = |v: i128| Value::Exact(crate::value::Decimal::new(v, 0));
        assert_eq!(octets.cmp(&n(2), &n(10)), Ordering::Less);
    }
}
