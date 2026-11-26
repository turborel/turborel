//! \[DESIGN\] (RULES.md D1206) The keys an inner hash join hashes, one a
//! column. Of `l.a = r.x AND l.a = r.y` both equalities were keys, so
//! each row hashed and compared the column `a` twice and the scan of `l`
//! was told two runtime filters of it. An equality whose left column or
//! whose right column is a kept key's already is judged of the matched
//! rows instead, in the filter over the join, which for an inner join is
//! its condition. The rows are the same, since no equality is dropped
//! here, and a moved one is true of no null, as a key is
//! [X3.135-1986 §5.11 GR2].
//!
//! The equalities are taken most selective first, by the greater of
//! their columns' distinct values, as the estimates count an equality's
//! pairs (D302), and of equal ones in the order stated. Outer joins, semi
//! joins, antijoins and mark joins keep every key, as they judge a
//! condition beyond their keys pair by pair inside the match.

use super::JoinKey;
use crate::logical::{Cond, Expr};
use crate::value::CompOp;

/// `keys` parted in the keys hashed, no two of one left column or of one
/// right column, in their order, and the ones judged after. `rank` gives
/// a key's distinct values, and is asked only if a column repeats.
pub(in crate::physical) fn single(keys: Vec<JoinKey>, rank: impl Fn(JoinKey) -> f64) -> (Vec<JoinKey>, Vec<JoinKey>) {
    if !repeats(&keys) {
        return (keys, Vec::new());
    }
    let mut ranked: Vec<(usize, f64)> = keys.iter().enumerate().map(|(i, &k)| (i, rank(k))).collect();
    // A stable sort, so equal ranks stay in the order stated.
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut taken: Vec<usize> = Vec::new();
    for (i, _) in ranked {
        let Some(&key) = keys.get(i) else { continue };
        if taken.iter().filter_map(|&t| keys.get(t)).all(|t| t.left != key.left && t.right != key.right) {
            taken.push(i);
        }
    }
    let (mut hashed, mut moved) = (Vec::new(), Vec::new());
    for (i, key) in keys.into_iter().enumerate() {
        if taken.contains(&i) {
            hashed.push(key);
        } else {
            moved.push(key);
        }
    }
    (hashed, moved)
}

/// Whether two of `keys` are of one left column or of one right column.
fn repeats(keys: &[JoinKey]) -> bool {
    keys.iter().enumerate().any(|(i, a)| keys.iter().skip(i.saturating_add(1)).any(|b| a.left == b.left || a.right == b.right))
}

/// `key` as the condition it is over the columns of its join's pairs, the
/// left operand `width` columns wide: an equality, or of a key on which a
/// null matches a null, NOT (left IS DISTINCT FROM right)
/// [ISO/IEC 9075-2:2023 §8.15 SR6].
pub(in crate::physical) fn judged(key: JoinKey, width: usize) -> Cond {
    let (left, right) = (Expr::Column(key.left), Expr::Column(key.right.saturating_add(width)));
    if key.nulls { Cond::Not(Box::new(Cond::Distinct(left, right))) } else { Cond::Compare(left, CompOp::Eq, right) }
}

/// Whether the equality `moved`, not hashed, is true of every pair the
/// hashed `keys` match. It is if one of them is of its left column and
/// the right operand's joins judged the two right columns equal, as
/// `rights` says, or of its right column and the left's joins judged the
/// two left columns equal, as `lefts` says. The key's equality and the
/// one judged below are each true, so of no null, and equality is
/// transitive [X3.135-1986 §5.11].
pub(in crate::physical) fn implied(
    keys: &[JoinKey],
    moved: JoinKey,
    lefts: impl Fn(usize, usize) -> bool,
    rights: impl Fn(usize, usize) -> bool,
) -> bool {
    let by_left = |k: &JoinKey| k.left == moved.left && rights(k.right, moved.right);
    let by_right = |k: &JoinKey| k.right == moved.right && lefts(k.left, moved.left);
    keys.iter().any(|k| by_left(k) || by_right(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of the left's column `left` and the right's `right`.
    fn key(left: usize, right: usize) -> JoinKey {
        JoinKey { left, right, nulls: false }
    }

    /// A rank of each key by its place in `keys`, as `ranks` gives.
    fn by(keys: &[JoinKey], ranks: &[f64]) -> impl Fn(JoinKey) -> f64 {
        let pairs: Vec<(JoinKey, f64)> = keys.iter().copied().zip(ranks.iter().copied()).collect();
        move |k| pairs.iter().find(|(p, _)| *p == k).map_or(0.0_f64, |&(_, r)| r)
    }

    /// Verifies: [DESIGN] RULES.md D1206: of keys of one left column, and
    /// of keys of one right column, one is hashed and the other judged
    /// after, the first stated where their ranks are equal.
    #[test]
    fn a_column_repeated_keeps_one_key() {
        let lefts = vec![key(0, 0), key(0, 1)];
        assert_eq!(single(lefts.clone(), by(&lefts, &[5.0_f64, 5.0_f64])), (vec![key(0, 0)], vec![key(0, 1)]));
        let rights = vec![key(0, 2), key(1, 2)];
        assert_eq!(single(rights.clone(), by(&rights, &[5.0_f64, 5.0_f64])), (vec![key(0, 2)], vec![key(1, 2)]));
    }

    /// Verifies: [DESIGN] RULES.md D1206, D302: the key of more distinct
    /// values is the one hashed, though stated last.
    #[test]
    fn the_key_of_most_distinct_values_is_kept() {
        let keys = vec![key(0, 0), key(0, 1)];
        assert_eq!(single(keys.clone(), by(&keys, &[5.0_f64, 50.0_f64])), (vec![key(0, 1)], vec![key(0, 0)]));
    }

    /// Verifies: [DESIGN] RULES.md D1206: a key that shares no column with
    /// a kept one is hashed too, the hashed keys in the order stated, so
    /// of `a = x AND b = y AND a = y` the first two are hashed whatever
    /// the third's rank is below theirs; and keys of no repeated column
    /// are all hashed, no rank asked.
    #[test]
    fn keys_sharing_no_column_are_all_kept() {
        let keys = vec![key(0, 0), key(1, 1), key(0, 1)];
        assert_eq!(single(keys.clone(), by(&keys, &[9.0_f64, 8.0_f64, 7.0_f64])), (vec![key(0, 0), key(1, 1)], vec![key(0, 1)]));
        // The third ranked first bars both others' columns.
        assert_eq!(single(keys.clone(), by(&keys, &[7.0_f64, 8.0_f64, 9.0_f64])), (vec![key(0, 1)], vec![key(0, 0), key(1, 1)]));
        let apart = vec![key(0, 0), key(1, 1)];
        assert_eq!(single(apart.clone(), |_| panic!("no rank is asked")), (apart, Vec::new()));
    }

    /// Verifies: [X3.135-1986 §5.11 GR2]; [ISO/IEC 9075-2:2023 §8.15 SR6];
    /// [DESIGN] RULES.md D1206: a key judged after is an equality of the
    /// pair's columns, true of no null, and one on which a null matches a
    /// null is NOT (IS DISTINCT FROM).
    #[test]
    fn a_moved_key_is_judged_as_it_matched() {
        assert!(matches!(judged(key(1, 2), 4_usize), Cond::Compare(Expr::Column(1), CompOp::Eq, Expr::Column(6))));
        let nulls = JoinKey { left: 1, right: 2, nulls: true };
        let Cond::Not(inner) = judged(nulls, 4_usize) else { panic!("an IS NOT DISTINCT FROM key is a negation") };
        assert!(matches!(*inner, Cond::Distinct(Expr::Column(1), Expr::Column(6))));
    }

    /// Verifies: [X3.135-1986 §5.11]; [DESIGN] RULES.md D1206, R-TEST-8: an
    /// equality not hashed is implied by a key of its left column whose
    /// right column the right's joins judged equal to its own (pair with
    /// the same key, the columns not judged equal, and with a key of
    /// another left column, they judged equal); or by a key of its right
    /// column whose left column the left's joins judged equal to its own
    /// (pairs likewise); and by no key otherwise.
    #[test]
    fn an_equality_is_implied_by_a_key_and_an_equality_judged_below() {
        let (yes, no) = (|_: usize, _: usize| true, |_: usize, _: usize| false);
        // The key (0, 0) and the equality (0, 1) share the left column.
        assert!(implied(&[key(0, 0)], key(0, 1), no, |a, b| (a, b) == (0, 1)));
        assert!(!implied(&[key(0, 0)], key(0, 1), yes, no));
        assert!(!implied(&[key(2, 0)], key(0, 1), no, yes));
        // The key (0, 0) and the equality (1, 0) share the right column.
        assert!(implied(&[key(0, 0)], key(1, 0), |a, b| (a, b) == (0, 1), no));
        assert!(!implied(&[key(0, 0)], key(1, 0), no, yes));
        assert!(!implied(&[key(0, 2)], key(1, 0), yes, no));
        assert!(!implied(&[], key(0, 1), yes, yes));
    }
}
