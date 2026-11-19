//! \[DESIGN\] A label index's numbering of its rows (RULES.md D115): each
//! row with an entry has a dense number, given in the order of the rows
//! as it is made of the tree and in the order they come after, and the
//! versions holding its entries counted. A row whose last entry is taken
//! back keeps its number, unused, until maintenance numbers the rows
//! again once more than an eighth of the numbers are unused.
//!
//! It is held of the database's memory budget (RULES.md D38), reserved
//! as it grows: one the budget refuses is dropped, the table then read in
//! its place, and made again only once as many rows were written as it
//! held, so maintenance does not read the index whole again for nothing.

use std::collections::HashMap;
use std::sync::Arc;

use crate::graph::indexes::MemberSet;
use crate::memory::{MemoryBudget, Reservation};
use crate::storage::RowId;

/// \[DESIGN\] Bytes a row is counted at: its id, number and count, its
/// share of a hash table, and the same again in the set read last.
const MEMBER_BYTES: usize = 96;
/// \[DESIGN\] Bytes reserved at a time as it grows, so the budget is asked
/// once per megabyte, not per row.
const RESERVE_STEP: usize = 1 << 20;
/// \[DESIGN\] The share of its numbers that may be unused before they are
/// given again: an eighth.
const DEAD_SHARE: usize = 8;
/// \[DESIGN\] The unused numbers below which none are given again,
/// whatever their share, so a small index is not numbered again for one.
pub(super) const LEAST_DEAD: usize = 64;
/// What a reservation of it is shown as.
const HOLDER: &str = "a label index's numbering of its rows";

/// The numbering, made.
struct Made {
    ids: HashMap<RowId, u32>,
    rows: Vec<RowId>,
    /// Of each number, the versions holding its row's entries.
    counts: Vec<u32>,
    /// The numbers whose count is 0.
    dead: usize,
    /// The set last given, until a row is written.
    snapshot: Option<Arc<MemberSet>>,
    held: Option<Reservation>,
}

/// \[DESIGN\] The numbering of a label index's rows, if made.
pub(super) struct Numbered {
    budget: Option<Arc<MemoryBudget>>,
    made: Option<Made>,
    /// The rows it held when the budget last refused it.
    refused: Option<usize>,
    /// The rows written since.
    written: usize,
}

impl Numbered {
    /// None yet, to be held of `budget`, if any.
    pub(super) fn new(budget: Option<Arc<MemoryBudget>>) -> Numbered {
        Numbered { budget, made: None, refused: None, written: 0 }
    }

    /// Whether it is to be made as it is read: neither made nor refused.
    pub(super) fn unmade(&self) -> bool {
        self.made.is_none() && self.refused.is_none()
    }

    /// Whether maintenance is to make it again: too many numbers unused,
    /// or, refused, as many rows written as it held.
    pub(super) fn due(&self) -> bool {
        match (&self.made, self.refused) {
            (Some(made), _) => made.dead > LEAST_DEAD.max(made.rows.len().checked_div(DEAD_SHARE).unwrap_or_default()),
            (None, Some(rows)) => self.written >= rows.max(LEAST_DEAD),
            (None, None) => false,
        }
    }

    /// Drops it, the budget having refused `rows` rows.
    fn refuse(&mut self, rows: usize) {
        self.made = None;
        self.refused = Some(rows);
        self.written = 0;
    }

    /// Makes it of `counts`, each row and the versions holding its
    /// entries, in the order of the rows; dropped if the budget refuses.
    pub(super) fn build(&mut self, counts: Vec<(RowId, u32)>) {
        let rows: Vec<(RowId, u32)> = counts.into_iter().filter(|(_, n)| *n > 0).collect();
        let bytes = rows.len().saturating_mul(MEMBER_BYTES).saturating_add(RESERVE_STEP);
        let held = match self.budget.as_ref().map(|b| b.reserve(bytes, HOLDER)) {
            Some(Ok(r)) => Some(r),
            Some(Err(_)) => return self.refuse(rows.len()),
            None => None,
        };
        let ids = rows.iter().zip(0_u32..).map(|(&(r, _), id)| (r, id)).collect();
        let made = Made {
            ids,
            counts: rows.iter().map(|&(_, n)| n).collect(),
            rows: rows.into_iter().map(|(r, _)| r).collect(),
            dead: 0,
            snapshot: None,
            held,
        };
        self.made = Some(made);
        self.refused = None;
    }

    /// One more version of `row`'s entries written.
    pub(super) fn added(&mut self, row: RowId) {
        self.written = self.written.saturating_add(1);
        let Some(made) = self.made.as_mut() else { return };
        made.snapshot = None;
        let next = u32::try_from(made.rows.len()).unwrap_or(u32::MAX);
        let id = *made.ids.entry(row).or_insert(next);
        if id == next {
            made.rows.push(row);
            made.counts.push(0);
        }
        if let Some(n) = made.counts.get_mut(usize::try_from(id).unwrap_or(usize::MAX)) {
            if *n == 0 && id != next {
                made.dead = made.dead.saturating_sub(1);
            }
            *n = n.saturating_add(1);
        }
        let wanted = made.rows.len().saturating_mul(MEMBER_BYTES);
        let short = made.held.as_ref().is_some_and(|h| h.bytes() < wanted);
        if short && made.held.as_mut().is_some_and(|h| h.resize(wanted.saturating_add(RESERVE_STEP)).is_err()) {
            let rows = made.rows.len();
            self.refuse(rows);
        }
    }

    /// One version of `row`'s entries taken back.
    pub(super) fn removed(&mut self, row: RowId) {
        let Some(made) = self.made.as_mut() else { return };
        let Some(&id) = made.ids.get(&row) else { return };
        made.snapshot = None;
        if let Some(n) = made.counts.get_mut(usize::try_from(id).unwrap_or(usize::MAX)) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                made.dead = made.dead.saturating_add(1);
            }
        }
    }

    /// The rows numbered now, if made; shared until a row is written.
    pub(super) fn snapshot(&mut self) -> Option<Arc<MemberSet>> {
        let made = self.made.as_mut()?;
        if let Some(set) = &made.snapshot {
            return Some(Arc::clone(set));
        }
        let live: Vec<bool> = made.counts.iter().map(|&n| n > 0).collect();
        let set = Arc::new(MemberSet::new(made.rows.clone(), &live));
        made.snapshot = Some(Arc::clone(&set));
        Some(set)
    }
}
