//! \[DESIGN\] (RULES.md D1267) A filter's conjunct judged of a column its
//! scan gave measured, late with its values' octet lengths and no text
//! (`storage::Late::lengths`). One conjunct is told by the lengths
//! alone. A comparison of a column's octets with the empty string's,
//! neither padded, as a collation of NO PAD that orders strings as their
//! octets do compares them [ISO/IEC 9075-2:2023 §4.3.2] (D1183), orders
//! a string of no octets equal to it and any other after it. So it is
//! the comparison of the string's octet length with 0, and of a null
//! unknown.
//!
//! That holds for no other conjunct. A comparison that pads with spaces
//! [X3.135-1986 §5.11 GR5] finds a string of spaces equal to the empty
//! string, whatever its length, and one with another constant needs the
//! octets. Each gives none here, and the filter then reads the column's
//! values, which decodes it as a late column read before it is resolved
//! is decoded (`storage/batch/late.rs`).

use super::Judged;
use crate::storage::Late;
use crate::storage::mask::index;
use crate::storage::sel::rows_of;

/// The rows of `over`, or of all, of the measured column `late` that
/// `judged` is true of, ascending, if it compares the column unpadded
/// with the empty string. None of any other conjunct, or of a column
/// its scan did not measure.
pub(super) fn kept(judged: &Judged, late: &Late, over: Option<&[u32]>) -> Option<Vec<u32>> {
    let Judged::Unpadded(_, op, octets) = judged else { return None };
    if !octets.is_empty() {
        return None;
    }
    let lengths = late.lengths()?;
    // A string's octets order against none as its length does against 0.
    let keeps = |r: &u32| !lengths.is_null(index(*r)) && lengths.units.get(index(*r)).is_some_and(|n| op.holds(n.cmp(&0)));
    Some(match over {
        Some(rows) => rows.iter().copied().filter(keeps).collect(),
        None => rows_of(0..lengths.units.len()).into_iter().filter(keeps).collect(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::error::Result;
    use crate::storage::{Column, Ints, LateSource, Mask};
    use crate::value::CompOp;

    /// A source asked for nothing here.
    struct Unread;

    impl LateSource for Unread {
        fn decode(&self, _column: usize, positions: &[u32]) -> Result<Column> {
            Ok(Column::nulls(positions.len()))
        }
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §4.3.2] RULES.md D1267: a
    /// comparison unpadded with the empty string keeps of a measured
    /// column the rows whose length compares so with 0, for each
    /// operator, a null never, of every row and of the rows given; a
    /// comparison with another constant, a conjunct of another kind and
    /// a late column not measured give none, for the filter to read the
    /// values. MC/DC: the empty constant against another; measured
    /// against not.
    #[test]
    fn the_empty_string_is_compared_by_lengths_alone() {
        let source: Arc<dyn LateSource> = Arc::new(Unread);
        let lengths = Ints::new(vec![3, 0, 0, 7, 1], 0, Some(Mask::from_bools(&[false, false, true, false, false])));
        let late = Late::new(Arc::clone(&source), 2, vec![10, 11, 12, 13, 14].into()).measured(lengths);
        let empty = |op: CompOp| Judged::Unpadded(0, op, Vec::new());
        let cases: [(CompOp, &[u32]); 6] = [
            (CompOp::Ne, &[0, 3, 4]),
            (CompOp::Eq, &[1]),
            (CompOp::Gt, &[0, 3, 4]),
            (CompOp::Ge, &[0, 1, 3, 4]),
            (CompOp::Lt, &[]),
            (CompOp::Le, &[1]),
        ];
        for (op, wanted) in cases {
            assert_eq!(kept(&empty(op), &late, None).as_deref(), Some(wanted), "{op:?}");
        }
        assert_eq!(kept(&empty(CompOp::Ne), &late, Some(&[1, 2, 4])), Some(vec![4]), "of the rows given");
        assert_eq!(kept(&Judged::Unpadded(0, CompOp::Ne, b"a".to_vec()), &late, None), None, "another constant needs the octets");
        assert_eq!(kept(&Judged::IsNull(0, false), &late, None), None, "another conjunct");
        let plain = Late::new(source, 2, vec![10, 11].into());
        assert_eq!(kept(&empty(CompOp::Ne), &plain, None), None, "not measured");
    }
}
