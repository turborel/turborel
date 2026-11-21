//! \[DESIGN\] (RULES.md D600) The runtime filters judged of one stream,
//! in the order of the share of its rows each kept so far, the one that
//! kept the least first, as Zhu et al. order a scan's filters by their
//! observed misses between batches (PVLDB 2017, "Looking Ahead Makes
//! Query Plans Robust", §4). A filter judges only the rows those before
//! it kept, so the one that drops the most spares the others the most
//! work. Each judges its rows as `RuntimeFilter::kept` does, whatever
//! its place, so their order changes the work and never the rows kept.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::error::Result;
use crate::exec::table::RuntimeFilter;
use crate::kernels::Kernels;
use crate::storage::{Batch, Column, Sel};

/// A filter applied to a stream's batches, and the rows it was given and
/// kept so far.
pub(super) struct Placed {
    pub(super) filter: Arc<RuntimeFilter>,
    /// The place of its column among a batch's, and whether it was moved
    /// across an equality of keys.
    pub(super) at: (usize, bool),
    /// What it judges, its range and its Bloom filter (`runtime::asked`).
    pub(super) asked: (bool, bool),
    given: u64,
    kept: u64,
}

impl Placed {
    /// `filter`, of the column at `at`, judging `asked`, given no row yet.
    pub(super) fn new(filter: Arc<RuntimeFilter>, at: (usize, bool), asked: (bool, bool)) -> Placed {
        Placed { filter, at, asked, given: 0, kept: 0 }
    }

    /// Whether it kept a smaller share of the rows it was given than
    /// `other` did, a filter given none keeping all of them.
    fn keeps_less(&self, other: &Placed) -> Ordering {
        let share = |p: &Placed| if p.given == 0 { (1, 1) } else { (u128::from(p.kept), u128::from(p.given)) };
        let ((kept, given), (other_kept, other_given)) = (share(self), share(other));
        kept.saturating_mul(other_given).cmp(&other_kept.saturating_mul(given))
    }
}

/// The rows of `batch` each of `placed` keeps, judged in turn of those
/// `rows` gives and those the filters before it kept, each while it
/// judges, a filter moved across an equality only of exact numbers,
/// whose equality is their value's whatever their types; then `placed`
/// put in the order of the share each kept, least first.
///
/// # Errors
///
/// A filter's (`RuntimeFilter::kept`).
pub(super) fn judged(placed: &mut [Placed], (batch, mut rows): (&Batch, Option<Sel>), kernels: &dyn Kernels) -> Result<Option<Sel>> {
    for p in placed.iter_mut() {
        let (at, moved) = p.at;
        if p.filter.judging() && (!moved || matches!(batch.columns.get(at), Some(Column::Ints(_)))) {
            let given = rows.as_deref().map_or(batch.rows, <[u32]>::len);
            let kept = p.filter.kept((batch, at), (rows.as_deref(), p.asked), kernels)?;
            p.given = p.given.saturating_add(u64::try_from(given).unwrap_or(u64::MAX));
            p.kept = p.kept.saturating_add(u64::try_from(kept.len()).unwrap_or(u64::MAX));
            rows = Some(Sel::of(kept));
        }
    }
    placed.sort_by(Placed::keeps_less);
    Ok(rows)
}

// Unit tests: the order filters are judged in (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::table::{Hashing, KeyHashes, Seed};
    use crate::kernels::Portable;
    use crate::storage::Ints;

    /// A filter of the built keys `keys`, of column 0, asked its range
    /// and its Bloom filter.
    fn placed(keys: &[i64]) -> Placed {
        let seed = Seed::fixed(7);
        let how = Hashing { seed: &seed, keyed: false };
        let built = Batch { columns: vec![Column::Ints(Ints::new(keys.to_vec(), 0, None))], rows: keys.len() };
        let mut hashes = KeyHashes::default();
        hashes.of_batch((&built, None), (&[0], &[false]), how).unwrap();
        Placed::new(Arc::new(RuntimeFilter::of_built(&built, (0, false), &hashes, how).unwrap()), (0, false), (true, true))
    }

    /// Verifies: [DESIGN] RULES.md D600: of two filters, the one that
    /// keeps fewer rows is judged first after a batch, the rows kept
    /// those both keep in either order; a filter given no row keeps all.
    #[test]
    fn the_filter_that_keeps_least_goes_first() {
        let batch = Batch { columns: vec![Column::Ints(Ints::new((0..100).collect(), 0, None))], rows: 100 };
        let (wide, narrow) = (placed(&(0..80).collect::<Vec<_>>()), placed(&[3, 5, 7]));
        let mut both = vec![wide, narrow];
        let first = judged(&mut both, (&batch, None), &Portable).unwrap();
        assert_eq!(first.as_deref(), Some(&[3_u32, 5, 7][..]));
        assert_eq!((both[0].given, both[0].kept), (80, 3), "the narrow filter, given the wide one's rows, first");
        let again = judged(&mut both, (&batch, None), &Portable).unwrap();
        assert_eq!(again.as_deref(), Some(&[3_u32, 5, 7][..]), "the rows kept alike in either order");
        assert_eq!(both[0].given, 180, "the narrow filter judged first: given all 100 rows");
        let fresh = placed(&[1]);
        assert_eq!(fresh.keeps_less(&both[0]), Ordering::Greater, "a filter given no row keeps all");
    }
}
