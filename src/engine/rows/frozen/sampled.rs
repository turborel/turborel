//! \[DESIGN\] The frozen rows a pass over a table reads (RULES.md D31,
//! D473, D1092): every one, or of a sampled pass those of the runs of row
//! ids it chose. A sampled pass costs its sample, not the table: a group
//! none of whose row ids is in a run chosen is passed over with its header
//! unread, and of a group whose rows are every id of its range in order
//! the places of a run's rows are reckoned from its ids, none of the
//! group's ids listed. Of a group in another order its ids are read, and
//! those of the runs chosen picked out.

use super::merge::VECTOR_ROWS;
use super::{Directory, Entry};
use crate::engine::columns::scan::rows;
use crate::engine::rows::{RowTable, corrupt, key};
use crate::error::Result;
use crate::storage::{Row, RowId};

/// The runs of row ids a sampled pass chose: their numbers, in order, each
/// run the `ids` row ids from its number times `ids`.
#[derive(Clone, Copy)]
pub(in crate::engine::rows) struct Chosen<'a> {
    pub(in crate::engine::rows) runs: &'a [u64],
    pub(in crate::engine::rows) ids: u64,
}

impl Chosen<'_> {
    /// Whether row `id` is in a run chosen.
    fn holds(&self, id: RowId) -> bool {
        id.checked_div(self.ids).is_some_and(|run| self.runs.binary_search(&run).is_ok())
    }

    /// The row ids, first and last, of each run chosen that lie from
    /// `first` to `last`, in order.
    fn within(&self, (first, last): (RowId, RowId)) -> Vec<(RowId, RowId)> {
        let end = |run: u64| run.saturating_add(1).saturating_mul(self.ids);
        let from = self.runs.partition_point(|&run| end(run) <= first);
        let found = self.runs.get(from..).unwrap_or_default().iter().take_while(|&&run| run.saturating_mul(self.ids) <= last);
        found.map(|&run| (run.saturating_mul(self.ids).max(first), end(run).saturating_sub(1).min(last))).collect()
    }
}

impl RowTable {
    /// Gives `each` every frozen row of `directory` no entry of the tree
    /// stands for, or of a sampled pass those of the runs `chosen`, every
    /// column of it, a vector at a time. Only the runs of rows chosen are
    /// decoded, and only the headers of their groups read (see the
    /// module's documentation).
    pub(in crate::engine::rows) fn each_unshadowed(
        &self,
        directory: &Directory,
        chosen: Option<Chosen<'_>>,
        mut each: impl FnMut(RowId, Row) -> Result<()>,
    ) -> Result<()> {
        for entry in &directory.groups {
            let range = (entry.summary.first, entry.summary.last);
            let wanted = chosen.map(|c| c.within(range));
            if wanted.as_ref().is_some_and(Vec::is_empty) {
                continue;
            }
            self.each_of_group(entry, (chosen, wanted.unwrap_or_else(|| vec![range])), &mut each)?;
        }
        Ok(())
    }

    /// Gives `each` the rows of `entry`'s group in `wanted`, ranges of row
    /// ids, first and last, in order, the runs `chosen` hold of it, or
    /// its range whole.
    fn each_of_group(
        &self,
        entry: &Entry,
        (chosen, wanted): (Option<Chosen<'_>>, Vec<(RowId, RowId)>),
        each: &mut impl FnMut(RowId, Row) -> Result<()>,
    ) -> Result<()> {
        let types = self.format.types();
        let all: Vec<usize> = (0..types.len()).collect();
        let group = self.group_of(entry, Some(&all))?;
        let pages = self.pages_for(&group)?;
        let place = |n: u64| u32::try_from(n).map_err(|_| corrupt("a group is too large"));
        // Each span of positions to decode, with its rows' ids.
        let mut spans: Vec<((u32, u32), Vec<RowId>)> = Vec::new();
        if group.ids.is_empty() {
            // Every id of its range, in order: a row's place is its id
            // past the first.
            for (first, last) in wanted {
                let mut from = first;
                while from <= last {
                    let to = from.saturating_add(u64::try_from(VECTOR_ROWS).unwrap_or(u64::MAX)).min(last.saturating_add(1));
                    spans.push(((place(from.saturating_sub(group.first))?, place(to.saturating_sub(group.first))?), (from..to).collect()));
                    from = to;
                }
            }
        } else {
            let ids = group.row_ids(&pages)?;
            for (from, to) in kept_runs(&ids, &|id| chosen.is_none_or(|c| c.holds(id))) {
                let span = (place(u64::try_from(from).unwrap_or(u64::MAX))?, u32::try_from(to).unwrap_or(u32::MAX));
                spans.push((span, ids.get(from..to).unwrap_or_default().to_vec()));
            }
        }
        for (span, ids) in spans {
            for (id, row) in ids.into_iter().zip(rows(&group, &pages, types, &all, span)?) {
                self.unless_shadowed((entry, id), row, each)?;
            }
        }
        Ok(())
    }

    /// Gives `each` frozen row `id`, `row`, unless an entry of the tree
    /// stands for it.
    fn unless_shadowed(&self, (entry, id): (&Entry, RowId), row: Row, each: &mut impl FnMut(RowId, Row) -> Result<()>) -> Result<()> {
        if self.may_shadow(entry, id) && self.tree.get(&key(id))?.is_some() {
            return Ok(());
        }
        each(id, row)
    }
}

/// The spans of positions in `ids` of rows `keep` keeps, each a run of
/// rows kept next to each other, of at most a vector of them.
pub(super) fn kept_runs(ids: &[RowId], keep: &impl Fn(RowId) -> bool) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (at, _) in ids.iter().enumerate().filter(|(_, id)| keep(**id)) {
        let next = at.saturating_add(1);
        let joined = runs.last_mut().filter(|run| run.1 == at && at.saturating_sub(run.0) < VECTOR_ROWS);
        if let Some(run) = joined {
            run.1 = next;
        } else {
            runs.push((at, next));
        }
    }
    runs
}

// Unit tests: the row ids of the runs a sampled pass chose that lie in a
// group's range (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::Chosen;

    /// Verifies: [DESIGN] RULES.md D1092, the runs chosen of a group: of
    /// runs of 128 row ids, those overlapping a range of ids are given,
    /// each cut to the range, in order; a range between runs holds none;
    /// and a row id is held by its run alone. Boundaries: a range ending
    /// on a run's first id, and one beginning on a run's last.
    #[test]
    fn the_runs_chosen_are_cut_to_a_groups_range() {
        let chosen = Chosen { runs: &[1, 2, 5], ids: 128 };
        assert_eq!(chosen.within((0, 127)), vec![]);
        assert_eq!(chosen.within((0, 128)), vec![(128, 128)], "ending on a run's first id");
        assert_eq!(chosen.within((200, 700)), vec![(200, 255), (256, 383), (640, 700)]);
        assert_eq!(chosen.within((383, 639)), vec![(383, 383)], "beginning on a run's last id");
        assert_eq!(chosen.within((384, 639)), vec![]);
        assert_eq!(chosen.within((768, 9000)), vec![]);
        assert!(chosen.holds(128) && chosen.holds(383) && chosen.holds(640));
        assert!(!chosen.holds(127) && !chosen.holds(384) && !chosen.holds(768));
    }
}
