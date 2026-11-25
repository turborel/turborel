//! \[DESIGN\] (RULES.md D1084) The rows of integers known as runs a test
//! of their units keeps, judged once a run. The runs and the rows are
//! walked together, both ascending, so each run is met once and each row
//! once: a run's rows found by stepping past them, no list of every row
//! made first when all are asked of, a run none of whose rows is asked of
//! not judged, and a run kept given on as a span of places.

use super::Ints;
use super::mask::index;

impl Ints {
    /// \[DESIGN\] (RULES.md D1084) The rows of `given`, or of all, in
    /// order, whose unit `keeps` is true, judged once a run of one unit
    /// (`with_runs`), its rows kept or not together, a null never: none if
    /// these are not known as runs, or `given` is not in increasing order.
    pub fn kept_by_runs(&self, given: Option<&[u32]>, mut keeps: impl FnMut(i64) -> bool) -> Option<Vec<u32>> {
        let ends = self.runs.as_deref()?;
        if given.is_some_and(|rows| !rows.is_sorted()) {
            return None;
        }
        let mut out = Vec::with_capacity(given.map_or(self.units.len(), <[u32]>::len));
        let (mut rest, mut start) = (given, 0_usize);
        for &end in ends {
            let these = match rest {
                Some(rows) => {
                    let (these, after) = rows.split_at(rows.iter().position(|&r| index(r) >= end).unwrap_or(rows.len()));
                    rest = Some(after);
                    these
                }
                None => &[],
            };
            let asked = rest.is_none() || !these.is_empty();
            if asked && keeps(self.units.get(start).copied().unwrap_or(0)) {
                self.push_run((start, end), given.map(|_| these), &mut out);
            }
            if rest.is_some_and(<[u32]>::is_empty) {
                break;
            }
            start = end;
        }
        Some(out)
    }

    /// Pushes to `out` the rows of the run `start` to `end` that are not
    /// null: of `these` alone, if given, else every place of it.
    fn push_run(&self, (start, end): (usize, usize), these: Option<&[u32]>, out: &mut Vec<u32>) {
        let places = || u32::try_from(start).unwrap_or(u32::MAX)..u32::try_from(end).unwrap_or(u32::MAX);
        match (these, &self.nulls) {
            (Some(rows), None) => out.extend_from_slice(rows),
            (Some(rows), Some(nulls)) => out.extend(rows.iter().filter(|&&r| !nulls.get(index(r)))),
            (None, None) => out.extend(places()),
            (None, Some(nulls)) => out.extend(places().filter(|&r| !nulls.get(index(r)))),
        }
    }
}

// Unit tests: rows kept by runs against the rows judged one at a time
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::super::mask::index;
    use super::super::{Ints, Mask};

    /// Runs of 3, 1, 4 and 2 rows of units 5, 7, 5 and 9, a null at row 5
    /// if `null`.
    fn runs(null: bool) -> Ints {
        let units = vec![5, 5, 5, 7, 5, 5, 5, 5, 9, 9];
        let nulls = null.then(|| Mask::from_bools(&(0_u32..10).map(|r| r == 5).collect::<Vec<_>>()));
        Ints::new(units, 0, nulls).with_runs(vec![3, 4, 8, 10])
    }

    /// The rows of `given`, or of all, whose unit `keeps`, judged one at
    /// a time, a null never.
    fn one_by_one(ints: &Ints, given: Option<&[u32]>, keeps: impl Fn(i64) -> bool) -> Vec<u32> {
        let all: Vec<u32> = (0..10).collect();
        let rows = given.unwrap_or(&all);
        rows.iter().copied().filter(|&r| !ints.is_null(index(r)) && ints.units.get(index(r)).is_some_and(|&u| keeps(u))).collect()
    }

    /// Verifies: [DESIGN] RULES.md D1084: rows kept by runs are the rows
    /// judged one at a time, of all rows and of some, a null never kept;
    /// a run none of whose rows is asked of is not judged; and rows out of
    /// order, or integers not known as runs, are refused.
    #[test]
    fn rows_kept_by_runs_are_the_rows_judged_one_at_a_time() {
        let fives = |u: i64| u == 5;
        for null in [false, true] {
            let ints = runs(null);
            for given in [None, Some(&[0_u32, 2, 3, 5, 9][..]), Some(&[4_u32, 5, 6][..]), Some(&[][..])] {
                assert_eq!(ints.kept_by_runs(given, fives), Some(one_by_one(&ints, given, fives)), "{given:?} null {null}");
                assert_eq!(ints.kept_by_runs(given, |_| true), Some(one_by_one(&ints, given, |_| true)), "{given:?}");
            }
        }
        let mut judged = Vec::new();
        let kept = runs(false).kept_by_runs(Some(&[1, 9]), |u| {
            judged.push(u);
            true
        });
        assert_eq!(kept, Some(vec![1, 9]));
        assert_eq!(judged, [5, 9], "the runs of 7 and of the second 5 are asked of by no row");
        assert_eq!(runs(false).kept_by_runs(Some(&[4, 0]), fives), None, "rows out of order");
        assert_eq!(Ints::new(vec![5, 5], 0, None).kept_by_runs(None, fives), None, "not known as runs");
    }
}
