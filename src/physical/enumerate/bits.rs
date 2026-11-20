//! \[DESIGN\] Sets of a join graph's relations as the bits of one integer,
//! relation `r` the bit `r` (Neumann and Radke, "Adaptive Optimization of
//! Very Large Join Queries", 2018, §5.1): a union or an intersection is
//! then one instruction, and the order of the relations, which DPccp and
//! DPhyp enumerate by, is the order of the bits.

/// A set of relations, by their places.
pub(super) type Set = u128;

/// The most relations a set holds: the bits of `Set`. A chain of more is
/// ordered as before (`physical/order.rs`).
pub(super) const MAX_RELATIONS: usize = 128;

/// The set of relation `r` alone; empty past `MAX_RELATIONS`.
pub(super) fn bit(r: usize) -> Set {
    u32::try_from(r).ok().and_then(|at| 1_u128.checked_shl(at)).unwrap_or(0)
}

/// The relations `0..=v`: those not after `v`, B_v of the papers.
pub(super) fn through(v: usize) -> Set {
    let next = bit(v.saturating_add(1));
    if next == 0 { Set::MAX } else { next.wrapping_sub(1) }
}

/// The first relation of `s`, if it has one: min(S) of the papers.
pub(super) fn lowest(s: Set) -> Option<usize> {
    (s != 0).then(|| usize::try_from(s.trailing_zeros()).unwrap_or(MAX_RELATIONS))
}

/// The set of the first relation of `s` alone, or empty.
pub(super) fn lowest_bit(s: Set) -> Set {
    s & s.wrapping_neg()
}

/// How many relations `s` holds.
pub(super) fn count(s: Set) -> usize {
    usize::try_from(s.count_ones()).unwrap_or(MAX_RELATIONS)
}

/// Whether `a` is a subset of `b`.
pub(super) fn within(a: Set, b: Set) -> bool {
    a & !b == 0
}

/// The relations of `s`, first to last.
pub(super) fn members(s: Set) -> impl Iterator<Item = usize> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let first = lowest(rest)?;
        rest &= rest.wrapping_sub(1);
        Some(first)
    })
}

/// The relations of `s`, last to first: the order DPccp and DPhyp take
/// their start nodes in.
pub(super) fn members_down(s: Set) -> impl Iterator<Item = usize> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let last = usize::try_from(Set::BITS.checked_sub(rest.leading_zeros())?.checked_sub(1)?).ok()?;
        rest &= !bit(last);
        Some(last)
    })
}

/// The subsets of `s` but the empty one, in ascending order of their
/// bits, so each smaller subset comes before the larger ones holding it.
pub(super) fn subsets(s: Set) -> impl Iterator<Item = Set> {
    let mut sub: Set = 0;
    std::iter::from_fn(move || {
        sub = sub.wrapping_sub(s) & s;
        (sub != 0).then_some(sub)
    })
}

// Unit tests: the set operations the enumerations rest on (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D300: members come first to last and
    /// last to first, subsets in ascending order, and B_v holds the
    /// relations not after v.
    #[test]
    fn sets_iterate_in_relation_order() {
        let s = bit(1) | bit(4) | bit(6);
        assert_eq!(members(s).collect::<Vec<_>>(), [1, 4, 6]);
        assert_eq!(members_down(s).collect::<Vec<_>>(), [6, 4, 1]);
        assert_eq!(subsets(bit(0) | bit(2)).collect::<Vec<_>>(), [1, 4, 5]);
        assert_eq!(through(2), 0b111);
        assert_eq!(through(MAX_RELATIONS - 1), Set::MAX);
        assert_eq!(lowest(s), Some(1));
        assert_eq!(lowest(0), None);
        assert_eq!(lowest_bit(s), bit(1));
        assert_eq!(count(s), 3);
        assert!(within(bit(4), s));
        assert!(!within(bit(5), s));
        assert_eq!(bit(MAX_RELATIONS), 0);
    }
}
