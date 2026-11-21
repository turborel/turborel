//! \[DESIGN\] A stable merge sort of places, by a comparison that may find
//! two values not comparable (RULES.md D580). The standard library's sort
//! may panic on a comparison that is no total order (R-ERR-1), which a
//! comparison of values of kinds that do not compare would be; this one
//! never does, and says whether every comparison it made was of values
//! that compare, so the range join holds only an order it can search.

use std::cmp::Ordering;

/// `0..n` in the order `compare` gives, equal places in their own order;
/// none if `compare` found two values not comparable.
pub(super) fn sorted(n: usize, compare: &mut dyn FnMut(usize, usize) -> Option<Ordering>) -> Option<Vec<usize>> {
    let mut places: Vec<usize> = (0..n).collect();
    let mut spare: Vec<usize> = Vec::with_capacity(n);
    let mut run = 1_usize;
    while run < n {
        spare.clear();
        let mut start = 0_usize;
        while start < n {
            let middle = start.saturating_add(run).min(n);
            let end = middle.saturating_add(run).min(n);
            merge((places.get(start..middle)?, places.get(middle..end)?), &mut spare, compare)?;
            start = end;
        }
        std::mem::swap(&mut places, &mut spare);
        run = run.saturating_mul(2);
    }
    Some(places)
}

/// The places of `first` and `second`, each in order, merged onto `out`:
/// of equal ones, `first`'s before. None if two are not comparable.
fn merge(
    (first, second): (&[usize], &[usize]),
    out: &mut Vec<usize>,
    compare: &mut dyn FnMut(usize, usize) -> Option<Ordering>,
) -> Option<()> {
    let (mut a, mut b) = (first.iter().peekable(), second.iter().peekable());
    while let (Some(&&x), Some(&&y)) = (a.peek(), b.peek()) {
        if compare(y, x)? == Ordering::Less {
            out.push(y);
            b.next();
        } else {
            out.push(x);
            a.next();
        }
    }
    out.extend(a);
    out.extend(b);
    Some(())
}

/// The first place of `places` past those `below` is true of, which must
/// be a prefix of them: none if it is not known of one.
pub(super) fn first(places: std::ops::Range<usize>, below: &mut dyn FnMut(usize) -> Option<bool>) -> Option<usize> {
    let (mut low, mut high) = (places.start, places.end);
    while low < high {
        let middle = low.saturating_add(high.saturating_sub(low) >> 1);
        if below(middle)? {
            low = middle.saturating_add(1);
        } else {
            high = middle;
        }
    }
    Some(low)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D580: places are sorted stably by their
    /// values, and a comparison that finds two values not comparable makes
    /// none.
    #[test]
    fn sorts_stably_or_finds_values_not_comparable() {
        let values: [i32; 7] = [5, 3, 5, 1, 3, 9, 0];
        let order = sorted(values.len(), &mut |a, b| Some(values[a].cmp(&values[b]))).unwrap();
        assert_eq!(order, vec![6, 3, 1, 4, 0, 2, 5]);
        assert_eq!(sorted(0, &mut |_, _| None), Some(Vec::new()));
        assert_eq!(sorted(1, &mut |_, _| None), Some(vec![0]));
        assert_eq!(sorted(3, &mut |a, b| (a != 2 && b != 2).then(|| a.cmp(&b))), None);
    }

    /// Verifies: [DESIGN] RULES.md D580: the first place past a prefix is
    /// found by halving, none if a value is not comparable.
    #[test]
    fn finds_the_end_of_a_prefix() {
        let keys: [i32; 5] = [1, 2, 2, 4, 8];
        assert_eq!(first(0..5, &mut |i| Some(keys[i] < 2_i32)), Some(1));
        assert_eq!(first(0..5, &mut |i| Some(keys[i] <= 2_i32)), Some(3));
        assert_eq!(first(0..5, &mut |i| Some(keys[i] < 100_i32)), Some(5));
        assert_eq!(first(2..5, &mut |i| Some(keys[i] < 0_i32)), Some(2));
        assert_eq!(first(0..5, &mut |_| None), None);
        assert_eq!(first(3..3, &mut |_| None), Some(3));
    }
}
