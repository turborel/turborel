//! \[DESIGN\] Reading ahead of a scan of frozen row groups (RULES.md D30,
//! D31): as a scan takes up a group, the pages of the chunks it will read,
//! of the columns it reads holding rows of the runs it reads, in the order
//! it reads them, a vector of each column at a time, a page of the hybrid
//! layout once; then, as it reads
//! each vector, as many of them as the pool's window has room for, asked
//! of the pool (`BufferPool::ahead_pages`), so the database's read-ahead
//! job reads them while the scan decodes those before.

use super::group::{Group, Pages};
use crate::disk::buffer::Ahead;

/// \[DESIGN\] What a scan of frozen groups reads ahead: the pages of the
/// group it reads it has yet to ask for.
#[derive(Default)]
pub struct GroupAhead {
    ahead: Ahead,
    pages: Vec<u64>,
    asked: usize,
}

impl GroupAhead {
    /// Takes up `group`, read of `columns`, at the positions of `runs`, in
    /// order and apart: its pages to ask for, those of the chunks of
    /// `columns` holding rows of `runs`, by their first rows, then in the
    /// order of `columns`.
    pub fn take_up(&mut self, group: &Group, columns: &[usize], runs: &[(u32, u32)]) {
        let mut chunks: Vec<(u32, usize, u64)> = Vec::new();
        for (at, zones) in columns.iter().enumerate().filter_map(|(at, &c)| Some((at, group.columns.get(c)?))) {
            let read = zones.iter().filter(|zone| holds_any(runs, (zone.run.first, zone.run.rows)));
            chunks.extend(read.map(|zone| (zone.run.first, at, zone.run.page.0)));
        }
        chunks.sort_unstable();
        self.pages = chunks.into_iter().map(|(_, _, page)| page).collect();
        // A page of the hybrid layout holds a chunk of each column, of the
        // same rows: asked for once.
        self.pages.dedup();
        self.asked = 0;
    }

    /// Asks for the pages of the group it has yet to ask for, as many as
    /// the pool of `pages` has room for.
    pub fn step<S>(&mut self, pages: &Pages<'_, S>) {
        let Some(rest) = self.pages.get(self.asked..).filter(|rest| !rest.is_empty()) else { return };
        let used = pages.pool.ahead_pages(&mut self.ahead, pages.file, rest);
        self.asked = self.asked.saturating_add(used);
    }
}

/// Whether the rows from `first`, `rows` of them, hold a position of
/// `runs`, in order and apart.
fn holds_any(runs: &[(u32, u32)], (first, rows): (u32, u32)) -> bool {
    let end = first.saturating_add(rows);
    let at = runs.partition_point(|&(_, to)| to <= first);
    runs.get(at).is_some_and(|&(from, _)| from < end)
}

// Unit tests: the pages a group's scan reads, in order (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::pager::PageId;
    use crate::engine::columns::group::{Run, Zone};

    fn zone(first: u32, rows: u32, page: u64) -> Zone {
        Zone { run: Run::whole(first, rows, PageId(page)), nulls: 0, bounds: None, parts: Vec::new() }
    }

    /// Verifies: [DESIGN] RULES.md D30, D31: a group's scan asks for the
    /// pages of the chunks of the columns it reads holding rows of its
    /// runs, by their first rows then in its columns' order; a chunk of no
    /// run, and a column it does not read, are left out.
    #[test]
    fn a_group_is_read_ahead_in_the_order_it_is_read() {
        let group = Group {
            first: 0,
            last: 299,
            rows: 300,
            ids: Vec::new(),
            by_id: Vec::new(),
            columns: vec![
                vec![zone(0, 100, 10), zone(100, 100, 11), zone(200, 100, 12)],
                vec![zone(0, 150, 20), zone(150, 150, 21)],
                vec![zone(0, 300, 30)],
            ],
            order: Vec::new(),
            pax: false,
            header: Vec::new(),
        };
        let mut ahead = GroupAhead::default();
        ahead.take_up(&group, &[1, 0], &[(0, 50), (210, 220)]);
        assert_eq!(ahead.pages, vec![20_u64, 10, 21, 12]);
        ahead.take_up(&group, &[0, 9], &[]);
        assert_eq!(ahead.pages, Vec::<u64>::new(), "no run");
        assert!(holds_any(&[(5, 10)], (9, 1)));
        assert!(!holds_any(&[(5, 10)], (10, 5)));
        assert!(!holds_any(&[(5, 10)], (0, 5)));
    }
}
