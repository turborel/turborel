//! \[EXTENSION\] What an approximate search asks of an engine, from the
//! query's `Approximation` (the vector design document §9.2, R9.3, R9.4,
//! R13.2): the rows wanted, the candidate list of a graph (DiskANN's L,
//! HNSW's ef), DiskANN's beam width, IVF's probes, a radius.
//!
//! A query's own search list or probes, or VECTOR.SEARCH_LIST and
//! VECTOR.BEAM_WIDTH, which the planner puts there, come first. Else a
//! TARGET ACCURACY is mapped to them, \[DESIGN\] as the share of the
//! candidates a search may miss: a list of `first × 100 / (101 −
//! accuracy)`, about nine times the rows at 90%, fifty at 99%; probes in
//! the same proportion, a 90% target scanning about one list in nine,
//! near IVF's default of one in eight. Else the index's own option. A
//! widened search (R9.3) multiplies the list and the probes, and a
//! post-filter's selectivity divides the rows the engine is asked for.

use crate::index::{Approximation, FilterHint};

/// A percentage's whole.
const PERCENT: usize = 100;
/// One past it: an accuracy of 100% maps to a hundred times the rows.
const PAST_PERCENT: usize = 101;
/// The lists a search scans at an accuracy's share of `PERCENT`, divided
/// by this: about one in nine at 90%.
const PROBE_SHARE: usize = 80;
/// The most candidates a search asks for, as DISKANN.SEARCH_LIST's and
/// HNSW.EF_SEARCH's greatest.
pub(crate) const MOST_LIST: usize = 10_000;
/// The least share of rows a post-filter is taken to keep: below it the
/// rows asked would pass `MOST_LIST`.
const LEAST_SHARE: f64 = 0.001;

/// An approximate search's request, resolved but for the index's own
/// defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ask {
    /// The rows the query asks for.
    pub(crate) first: usize,
    /// The rows the engine is asked for: `first`, over a post-filter's
    /// share.
    pub(crate) wanted: usize,
    search_list: Option<u32>,
    /// DiskANN's W, if the query sets it.
    pub(crate) beam_width: Option<u32>,
    probes: Option<u32>,
    accuracy: Option<u8>,
    widen: u32,
    /// Every row within it, instead of the nearest.
    pub(crate) radius: Option<f64>,
    /// The share of rows the condition keeps, a percentage, of a post-filter.
    pub(crate) selectivity: Option<u32>,
}

impl Ask {
    /// What `a` asks.
    pub(crate) fn of(a: &Approximation) -> Ask {
        let first = a.first.max(1);
        let share = match a.filter {
            FilterHint::PostFilter(share) => Some(share.clamp(LEAST_SHARE, 1.0_f64)),
            FilterHint::None | FilterHint::InFilter => None,
        };
        let wanted = share.map_or(first, |s| rows_over(first, s));
        let selectivity = share.map(percent_of);
        Ask {
            first,
            wanted,
            search_list: a.search_list,
            beam_width: a.beam_width,
            probes: a.probes,
            accuracy: a.accuracy,
            widen: a.widen.max(1),
            radius: a.radius,
            selectivity,
        }
    }

    /// The candidate list: the query's, else its accuracy's, else the
    /// index's `own`; widened, at least the rows wanted, at most
    /// `MOST_LIST`.
    pub(crate) fn list(&self, own: usize) -> usize {
        let base = match (self.search_list, self.accuracy) {
            (Some(list), _) => usize::try_from(list).unwrap_or(MOST_LIST),
            (None, Some(accuracy)) => self.wanted.saturating_mul(PERCENT).checked_div(missed(accuracy)).unwrap_or(MOST_LIST),
            (None, None) => own,
        };
        base.saturating_mul(self.widening()).max(self.wanted).min(MOST_LIST.max(self.wanted))
    }

    /// The lists scanned of `lists`: the query's, else its accuracy's,
    /// else none for the index's own; widened, at most `lists`.
    pub(crate) fn probes(&self, lists: usize, own: usize) -> usize {
        let base = match (self.probes, self.accuracy) {
            (Some(probes), _) => usize::try_from(probes).unwrap_or(lists),
            (None, Some(accuracy)) => {
                lists.saturating_mul(PERCENT).checked_div(missed(accuracy).saturating_mul(PROBE_SHARE)).unwrap_or(lists).max(1)
            }
            (None, None) => own,
        };
        base.saturating_mul(self.widening()).clamp(1, lists.max(1))
    }

    fn widening(&self) -> usize {
        usize::try_from(self.widen).unwrap_or(usize::MAX)
    }
}

/// The share of candidates an accuracy lets a search miss, of `PAST_PERCENT`.
fn missed(accuracy: u8) -> usize {
    PAST_PERCENT.saturating_sub(usize::from(accuracy.min(100))).max(1)
}

/// `first` over `share`, rounded up, at most `MOST_LIST`.
fn rows_over(first: usize, share: f64) -> usize {
    let mut rows = first;
    // Doubled up to the share's inverse: no float converts to an integer
    // without a cast (R-NUM-2).
    let mut kept = share;
    while kept < 1.0_f64 && rows < MOST_LIST {
        rows = rows.saturating_mul(2);
        kept *= 2.0_f64;
    }
    rows.min(MOST_LIST.max(first))
}

/// How far a share times a hundred may sit above a whole percentage and
/// round to it: a product's rounding error.
const ROUNDING: f64 = 1e-9;

/// `share`, a fraction, as a whole percentage, at least 1.
fn percent_of(share: f64) -> u32 {
    (1_u32..=100).find(|p| f64::from(*p) + ROUNDING >= share * 100.0_f64).unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(search_list: Option<u32>, accuracy: Option<u8>, widen: u32, filter: FilterHint) -> Ask {
        Ask::of(&Approximation { first: 10, search_list, beam_width: None, probes: None, accuracy, widen, radius: None, filter })
    }

    /// Verifies: [EXTENSION] vector_standard §9.2 R9.4 A query's search
    /// list comes first, then its TARGET ACCURACY, mapped to about nine
    /// times the rows at 90% and fifty at 99%, then the index's own; a
    /// widened search (R9.3) multiplies it; it is never below the rows
    /// asked nor past the greatest list.
    #[test]
    fn a_search_list_comes_from_the_query_its_accuracy_or_the_index() {
        assert_eq!(asked(Some(40), Some(99), 1, FilterHint::None).list(100), 40);
        assert_eq!(asked(None, Some(90), 1, FilterHint::None).list(100), 90);
        assert_eq!(asked(None, Some(99), 1, FilterHint::None).list(100), 500);
        assert_eq!(asked(None, None, 1, FilterHint::None).list(64), 64);
        assert_eq!(asked(None, None, 4, FilterHint::None).list(64), 256);
        assert_eq!(asked(Some(2), None, 1, FilterHint::None).list(64), 10);
        assert_eq!(asked(Some(9_000), None, 4, FilterHint::None).list(64), MOST_LIST);
    }

    /// Verifies: [EXTENSION] vector_standard §9.2 R9.4 IVF's probes come
    /// from the query, then its accuracy, about one list in nine at 90%,
    /// then the index's own; widened, at least one, at most every list.
    #[test]
    fn probes_come_from_the_query_its_accuracy_or_the_index() {
        let mut a = asked(None, Some(90), 1, FilterHint::None);
        assert_eq!(a.probes(900, 7), 102);
        a.probes = Some(3);
        assert_eq!(a.probes(900, 7), 3);
        assert_eq!(asked(None, None, 2, FilterHint::None).probes(900, 7), 14);
        assert_eq!(asked(None, None, 1, FilterHint::None).probes(4, 0), 1);
        assert_eq!(asked(None, Some(100), 8, FilterHint::None).probes(16, 7), 16);
    }

    /// Verifies: [EXTENSION] vector_standard R13.2 A post-filter keeping a
    /// share of the rows asks the engine for the rows over that share,
    /// rounded up to a power of two, and tells it the share as a
    /// percentage; an in-filter asks for the rows alone.
    #[test]
    fn a_post_filter_asks_for_more_rows() {
        let post = asked(None, None, 1, FilterHint::PostFilter(0.1_f64));
        assert_eq!((post.first, post.wanted, post.selectivity), (10, 160, Some(10)));
        let inside = asked(None, None, 1, FilterHint::InFilter);
        assert_eq!((inside.wanted, inside.selectivity), (10, None));
        let tiny = asked(None, None, 1, FilterHint::PostFilter(0.0_f64));
        assert!(tiny.wanted <= MOST_LIST && tiny.selectivity == Some(1));
    }
}
