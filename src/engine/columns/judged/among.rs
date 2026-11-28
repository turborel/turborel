//! \[DESIGN\] (RULES.md D1294) The keys of a hash join's runtime filter
//! asked by a column store's scan (`Conjunct::Among`, `storage::Members`).
//! A join tells its probing side's scan the keys its built rows hold. The
//! scan asks them of the key's column, a vector of integers, after the
//! conjuncts that cost less a row and before the other columns are
//! decoded: once a run of equal keys where the column is kept as runs,
//! else of each row. A run of which few rows are kept is then decoded at
//! those rows alone (`rows/frozen/batches/run.rs`), so the filter over
//! the scan, the join's own filter step and its probe see those rows and
//! no others of it.
//!
//! A row left out is one no built row matches, which the join drops. A
//! row kept is judged again by the join, so a Bloom filter's false hit
//! changes nothing the statement gives. The scan stops asking once its
//! first runs show the keys keep one row in `KEPT_SHARE` or more
//! (`Members::asking`), as a run given whole gains nothing by it.

use super::{Pred, rows};
use crate::storage::mask::index;
use crate::storage::{Conjunct, Ints, Members};

impl Pred {
    /// \[DESIGN\] (RULES.md D1294) The keys of a join's filter it asks,
    /// if it is one.
    pub(super) fn among(&self) -> Option<&Members> {
        if let Conjunct::Among { members, .. } = &self.conjunct { Some(members) } else { None }
    }

    /// \[DESIGN\] (RULES.md D1294) Counts, of a join's keys, a run they
    /// were asked of, of `asked` rows of which they kept `kept`.
    pub(super) fn counted(&self, asked: usize, kept: usize) {
        if let Some(members) = self.among() {
            members.count(asked, kept);
        }
    }
}

/// The rows of `given`, or of all, of `ints` whose key `members` may
/// hold, a null never, as the filter is of a key no null matches: asked
/// once a run of integers kept as runs (`Ints::kept_by_runs`), else of
/// each row.
pub(super) fn kept(members: &Members, ints: &Ints, given: Option<&[u32]>) -> Vec<u32> {
    let holds = |u: i64| members.may_hold(u, ints.scale);
    if ints.runs().is_some()
        && let Some(kept) = ints.kept_by_runs(given, holds)
    {
        return kept;
    }
    let unit = |r: u32| ints.units.get(index(r)).copied().filter(|_| !ints.is_null(index(r)));
    rows(ints.units.len(), given).into_iter().filter(|&r| unit(r).is_some_and(holds)).collect()
}

// Unit tests: the rows a join's keys keep of a vector and of a group's
// runs, and when the scan stops asking (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
