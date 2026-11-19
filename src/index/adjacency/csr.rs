//! \[DESIGN\] An adjacency index's compacted adjacency (RULES.md
//! D107): compressed sparse rows, built in memory from the
//! tree's entries in their order, by maintenance (`maintain.rs`) or as the
//! index is loaded (`load.rs`). Each vertex has a dense number, given as it
//! is first met; each direction kept has, by number, the span of its
//! vertex's steps in arrays of the other end's number, the edge's row and
//! the included columns' bytes. A vertex's steps are its entries in the
//! tree's order, so a list read here is the list the tree holds.
//!
//! It is held of the database's memory budget (RULES.md D38), reserved as
//! it grows, and of the index's ADJACENCY.CSR_BYTES: a build that would
//! pass either stops, and the index is read from its tree alone.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::Range;
use std::sync::Arc;

use super::keys::{Parsed, corrupt, decode_all};
use crate::error::Result;
use crate::graph::adjacency::{Direction, Step};
use crate::index::Key;
use crate::memory::{MemoryBudget, Reservation};
use crate::storage::RowId;
use crate::value::DataType;

/// \[DESIGN\] Bytes a vertex is counted at beside its key's: its key held
/// twice by reference, its number, its share of a hash table, and its
/// span in each direction.
const VERTEX_BYTES: usize = 96;
/// Bytes of a step: the other end's number and the row.
const STEP_BYTES: usize = 12;
/// Bytes of a step's included columns' end, of an index that has them.
const INCLUDED_END_BYTES: usize = 4;
/// \[DESIGN\] Bytes reserved at a time as a build grows, so the budget is
/// asked once per megabyte, not per step.
const RESERVE_STEP: usize = 1 << 20;
/// What a reservation of it is shown as.
const HOLDER: &str = "an adjacency index's compacted adjacency";

/// The steps of one direction, by vertex number.
#[derive(Default)]
struct Lists {
    /// Of each vertex's number, where its steps start and end.
    spans: Vec<(u32, u32)>,
    /// Of each step, the other end's number.
    others: Vec<u32>,
    /// Of each step, the edge's row.
    rows: Vec<RowId>,
    /// Of each step, where its included columns' bytes end, if any are.
    ends: Vec<u32>,
    /// The included columns' bytes of every step, one after another.
    included: Vec<u8>,
}

impl Lists {
    /// Where the steps of vertex `id` are.
    fn span(&self, id: u32) -> Range<usize> {
        let at = usize::try_from(id).unwrap_or(usize::MAX);
        let (start, end) = self.spans.get(at).copied().unwrap_or_default();
        usize::try_from(start).unwrap_or_default()..usize::try_from(end).unwrap_or_default()
    }

    /// The included columns' bytes of step `at`.
    fn included(&self, at: usize) -> &[u8] {
        let end = |i: Option<usize>| i.and_then(|i| self.ends.get(i)).and_then(|&e| usize::try_from(e).ok()).unwrap_or_default();
        let (start, stop) = (end(at.checked_sub(1)), end(Some(at)));
        self.included.get(start..stop).unwrap_or_default()
    }
}

/// \[DESIGN\] A compacted adjacency, fixed once built.
pub struct Csr {
    /// The number of each vertex, by its key's bytes.
    ids: HashMap<Arc<[u8]>, u32>,
    /// The key's bytes of each vertex, by number.
    names: Vec<Arc<[u8]>>,
    out: Lists,
    inward: Lists,
    /// The bytes it is counted at.
    bytes: usize,
    /// Its bytes of the budget, given back as it goes.
    held: Option<Reservation>,
}

impl Csr {
    /// The lists of `direction`.
    fn lists(&self, direction: Direction) -> &Lists {
        match direction {
            Direction::Out => &self.out,
            Direction::In => &self.inward,
        }
    }

    /// The vertices numbered.
    pub fn vertices(&self) -> usize {
        self.names.len()
    }

    /// The bytes it is counted at.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Where the steps of the vertex of key bytes `vertex` are in
    /// `direction`: none of a vertex it has no number for, which has none.
    pub fn span(&self, direction: Direction, vertex: &[u8]) -> Range<usize> {
        self.ids.get(vertex).map_or(0..0, |&id| self.lists(direction).span(id))
    }

    /// Step `at` of `direction`: the other end's key bytes, the row, and
    /// the included columns' bytes.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it has no such step, which a span never gives.
    pub fn step(&self, direction: Direction, at: usize) -> Result<(&[u8], RowId, &[u8])> {
        let lists = self.lists(direction);
        let other = lists.others.get(at).and_then(|&o| self.names.get(usize::try_from(o).ok()?)).ok_or_else(corrupt)?;
        let row = lists.rows.get(at).copied().ok_or_else(corrupt)?;
        Ok((other, row, lists.included(at)))
    }

    /// The number of the vertex of key bytes `vertex`, if it has one.
    pub fn number(&self, vertex: &[u8]) -> Option<u32> {
        self.ids.get(vertex).copied()
    }

    /// Pushes onto each list of `lists` the steps of its vertex in
    /// `direction`, as `steps` would, the vertices `numbered` (each a
    /// number and the place of its list) read in the order of their
    /// numbers, one pass over the arrays; each other end's key decoded
    /// once for all of them.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a key is not of the layout.
    pub fn steps_numbered(
        &self,
        direction: Direction,
        mut numbered: Vec<(u32, usize)>,
        other_types: &[DataType],
        lists: &mut [Vec<Step>],
    ) -> Result<()> {
        numbered.sort_unstable();
        let edges = self.lists(direction);
        let mut decoded: HashMap<u32, Key> = HashMap::new();
        for (id, place) in numbered {
            let Some(list) = lists.get_mut(place) else { continue };
            let mut previous: Option<(u32, RowId)> = None;
            for at in edges.span(id) {
                let other = edges.others.get(at).copied().ok_or_else(corrupt)?;
                let row = edges.rows.get(at).copied().ok_or_else(corrupt)?;
                if previous == Some((other, row)) {
                    continue;
                }
                previous = Some((other, row));
                let vertex = match decoded.entry(other) {
                    Entry::Occupied(known) => known.get().clone(),
                    Entry::Vacant(new) => {
                        let name = self.names.get(usize::try_from(other).unwrap_or(usize::MAX)).ok_or_else(corrupt)?;
                        new.insert(Key(decode_all(name, other_types)?)).clone()
                    }
                };
                list.push(Step { edge: row, vertex });
            }
        }
        Ok(())
    }

    /// Pushes onto `out` the steps of the vertex of key bytes `vertex` in
    /// `direction`, the other end's key of columns of `other_types`: one of
    /// the entries of an edge alike but for included columns, adjacent.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a key is not of the layout.
    pub fn steps(&self, direction: Direction, vertex: &[u8], other_types: &[DataType], out: &mut Vec<Step>) -> Result<()> {
        let mut last: Option<(&[u8], RowId)> = None;
        for at in self.span(direction, vertex) {
            let (other, row, _) = self.step(direction, at)?;
            if last == Some((other, row)) {
                continue;
            }
            last = Some((other, row));
            out.push(Step { edge: row, vertex: Key(decode_all(other, other_types)?) });
        }
        Ok(())
    }
}

/// \[DESIGN\] A compacted adjacency as it is built, of entries given in the
/// tree's order: every forward entry, then every reverse one.
pub struct Builder {
    csr: Csr,
    /// Its bytes of the budget, reserved ahead of what it counts.
    held: Reservation,
    /// The most bytes it may count.
    most: usize,
    /// The direction and vertex whose steps are being given, and where
    /// they start.
    open: Option<(Direction, u32, u32)>,
}

impl Builder {
    /// A builder of a compacted adjacency of at most `most` bytes, held of
    /// `budget`.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget has no room at all.
    pub fn new(budget: &Arc<MemoryBudget>, most: usize) -> Result<Builder> {
        let held = budget.reserve(0, HOLDER)?;
        let lists = (Lists::default(), Lists::default());
        let csr = Csr { ids: HashMap::new(), names: Vec::new(), out: lists.0, inward: lists.1, bytes: 0, held: None };
        Ok(Builder { csr, held, most, open: None })
    }

    /// Counts `bytes` more: whether they fit its most, and the budget.
    fn grow(&mut self, bytes: usize) -> bool {
        let Some(counted) = self.csr.bytes.checked_add(bytes).filter(|&n| n <= self.most) else { return false };
        self.csr.bytes = counted;
        counted <= self.held.bytes() || self.held.resize(counted.saturating_add(RESERVE_STEP)).is_ok()
    }

    /// The number of the vertex of key bytes `vertex`, given it if new:
    /// none if it does not fit.
    fn number(&mut self, vertex: &[u8]) -> Option<u32> {
        if let Some(&id) = self.csr.ids.get(vertex) {
            return Some(id);
        }
        let id = u32::try_from(self.csr.names.len()).ok()?;
        if !self.grow(VERTEX_BYTES.saturating_add(vertex.len())) {
            return None;
        }
        let name: Arc<[u8]> = Arc::from(vertex);
        self.csr.ids.insert(Arc::clone(&name), id);
        self.csr.names.push(name);
        Some(id)
    }

    /// Ends the open vertex's span.
    fn close(&mut self) {
        let Some((direction, id, start)) = self.open.take() else { return };
        let vertices = self.csr.names.len();
        let lists = match direction {
            Direction::Out => &mut self.csr.out,
            Direction::In => &mut self.csr.inward,
        };
        let end = u32::try_from(lists.others.len()).unwrap_or(u32::MAX);
        lists.spans.resize(vertices, (0, 0));
        if let Some(span) = lists.spans.get_mut(usize::try_from(id).unwrap_or(usize::MAX)) {
            *span = (start, end);
        }
    }

    /// Adds the entry `entry` as a step: whether it fit, the build
    /// stopped if not.
    pub fn push(&mut self, entry: &Parsed<'_>) -> bool {
        let (Some(vertex), Some(other)) = (self.number(entry.vertex), self.number(entry.other)) else { return false };
        let included = entry.included.len();
        let extra = if included == 0 { 0 } else { INCLUDED_END_BYTES };
        if !self.grow(STEP_BYTES.saturating_add(extra).saturating_add(included)) {
            return false;
        }
        if self.open.is_none_or(|(d, v, _)| (d, v) != (entry.direction, vertex)) {
            self.close();
            let start = u32::try_from(self.csr.lists(entry.direction).others.len());
            let Ok(start) = start else { return false };
            self.open = Some((entry.direction, vertex, start));
        }
        let lists = match entry.direction {
            Direction::Out => &mut self.csr.out,
            Direction::In => &mut self.csr.inward,
        };
        lists.others.push(other);
        lists.rows.push(entry.row);
        if included > 0 {
            lists.included.extend_from_slice(entry.included);
            let Ok(end) = u32::try_from(lists.included.len()) else { return false };
            lists.ends.push(end);
        }
        true
    }

    /// The compacted adjacency built, holding its bytes of the budget.
    pub fn finish(mut self) -> Csr {
        self.close();
        let vertices = self.csr.names.len();
        self.csr.out.spans.resize(vertices, (0, 0));
        self.csr.inward.spans.resize(vertices, (0, 0));
        let bytes = self.csr.bytes;
        // Down to what it counts: the reservation was ahead of it.
        let _ = self.held.resize(bytes);
        self.csr.held = Some(self.held);
        self.csr
    }
}
