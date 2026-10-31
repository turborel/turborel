//! \[DESIGN\] A list that spills (RULES.md D38): items appended, held in
//! memory until they pass a number of bytes, then written, the segments
//! held together as a run of a spill area made when first needed; read
//! back a segment at a time, newest or oldest first, or the segment of an
//! item's place. A mark begins a segment, so the items from it on are
//! taken apart from those before without reading a run back, unless they
//! were written in one run with them. Items that cannot be written stay held: a list never loses
//! one. What its items are in bytes is its owner's, given as it writes and
//! reads them: the transaction coordinator's undo entries, a row store's
//! changes and garbage, the row changes recovery reads from the log, the
//! rows a cursor or a data-changing statement holds (`exec/held.rs`).

use std::sync::Arc;

use super::{Area, SpillArea, SpillRun, WorkAreas};
use crate::error::{Code, Result, SqlError};
use crate::value::types::TypeRegistry;

/// Where a list spills: the bytes of items it holds, and the work areas,
/// with batches of `batch` bytes, its runs are made in; none to hold them
/// all.
#[derive(Clone)]
pub struct ListSpace {
    /// The bytes of items held before they are written.
    pub limit: usize,
    /// The bytes of records a run gathers before it writes them.
    pub batch: usize,
    /// The work areas, if it may spill.
    pub areas: Option<Arc<WorkAreas>>,
    /// The types beyond the standard its items may hold values of, read
    /// back by them once spilled.
    pub types: TypeRegistry,
}

impl Default for ListSpace {
    fn default() -> ListSpace {
        ListSpace { limit: usize::MAX, batch: 0, areas: None, types: TypeRegistry::default() }
    }
}

/// A part of a list: items held, or written to a run, with how many.
enum Segment<T> {
    Held(Vec<T>, usize),
    Spilled(Box<dyn SpillRun>, usize),
}

impl<T> Segment<T> {
    fn len(&self) -> usize {
        match self {
            Segment::Held(items, _) => items.len(),
            Segment::Spilled(_, n) => *n,
        }
    }

    fn held_bytes(&self) -> usize {
        match self {
            Segment::Held(_, bytes) => *bytes,
            Segment::Spilled(..) => 0,
        }
    }
}

/// \[DESIGN\] The list.
pub struct SpillList<T> {
    space: ListSpace,
    /// Its area, made when it first spills.
    area: Option<Box<dyn SpillArea>>,
    segments: Vec<Segment<T>>,
    held: usize,
    count: usize,
}

impl<T: ListItem> Default for SpillList<T> {
    fn default() -> SpillList<T> {
        SpillList::new(ListSpace::default())
    }
}

/// An item of a list: what it holds in memory, near enough.
pub trait ListItem {
    /// Its bytes in memory.
    fn bytes(&self) -> usize;
}

/// How a list's items are written: each item to its record.
pub type Encode<'a, T> = &'a mut dyn FnMut(&T) -> Vec<u8>;

/// How a list's items are read back: each record to its item.
pub type Decode<'a, T> = &'a (dyn Fn(&[u8]) -> Result<T> + Sync);

impl<T: ListItem> SpillList<T> {
    /// An empty list, spilling in `space`.
    pub fn new(space: ListSpace) -> SpillList<T> {
        SpillList { space, area: None, segments: Vec::new(), held: 0, count: 0 }
    }

    /// How many items it has: the place of the next.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The bytes of the items it holds in memory.
    pub fn held(&self) -> usize {
        self.held
    }

    /// Begins a segment, so the items from here on are apart from those
    /// before; their place.
    pub fn mark(&mut self) -> usize {
        let begun = match self.segments.last() {
            None => true,
            Some(Segment::Held(items, _)) => items.is_empty(),
            Some(Segment::Spilled(..)) => false,
        };
        if !begun {
            self.segments.push(Segment::Held(Vec::new(), 0));
        }
        self.count
    }

    /// Appends `item`, writing the segments held with `encode` once they
    /// pass the space's bytes.
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved: the item is
    /// appended all the same, held.
    pub fn push(&mut self, item: T, encode: Encode<'_, T>) -> Result<()> {
        let bytes = item.bytes();
        self.held = self.held.saturating_add(bytes);
        self.count = self.count.saturating_add(1);
        if let Some(Segment::Held(items, held)) = self.segments.last_mut() {
            items.push(item);
            *held = held.saturating_add(bytes);
        } else {
            self.segments.push(Segment::Held(vec![item], bytes));
        }
        if self.held > self.space.limit && self.space.areas.is_some() { self.spill(encode) } else { Ok(()) }
    }

    /// Writes the segments held to runs, those one after another together
    /// in one run, so segments of a few items each, a statement's, take a
    /// run, and its pages, between them rather than one each; a place
    /// among them is then parted by reading the run back (`take_from`).
    /// Once a run cannot be written, the rest stay held.
    fn spill(&mut self, encode: Encode<'_, T>) -> Result<()> {
        let (mut segments, mut failed) = (Vec::with_capacity(self.segments.len()), None);
        let mut gathered: (Vec<T>, usize) = (Vec::new(), 0);
        for segment in std::mem::take(&mut self.segments) {
            match segment {
                Segment::Held(items, bytes) => {
                    gathered.0.extend(items);
                    gathered.1 = gathered.1.saturating_add(bytes);
                }
                spilled @ Segment::Spilled(..) => {
                    segments.extend(self.gathered(std::mem::take(&mut gathered), encode, &mut failed));
                    segments.push(spilled);
                }
            }
        }
        segments.extend(self.gathered(gathered, encode, &mut failed));
        self.segments = segments;
        failed.map_or(Ok(()), Err)
    }

    /// The segment of `items`, of `bytes`: written to a run unless one
    /// has `failed`, else held, the error kept in `failed`; none if empty.
    fn gathered(&mut self, (items, bytes): (Vec<T>, usize), encode: Encode<'_, T>, failed: &mut Option<SqlError>) -> Option<Segment<T>> {
        if items.is_empty() {
            return None;
        }
        if failed.is_some() {
            return Some(Segment::Held(items, bytes));
        }
        match self.written(&items, encode) {
            Ok(run) => {
                self.held = self.held.saturating_sub(bytes);
                Some(Segment::Spilled(run, items.len()))
            }
            Err(e) => {
                *failed = Some(e);
                Some(Segment::Held(items, bytes))
            }
        }
    }

    /// `items` in a new run of its area, made the first time. A list that
    /// lives long, as a transaction's undo entries, keeps its area: its
    /// pages are written through the encodings in use as each is written,
    /// so encrypted once anything is (RULES.md D48).
    fn written(&mut self, items: &[T], encode: Encode<'_, T>) -> Result<Box<dyn SpillRun>> {
        let areas = self.space.areas.as_ref().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a list that spills has no work areas"))?;
        if self.area.is_none() {
            self.area = Some(areas.area(Area::new(self.space.limit, self.space.batch))?);
        }
        let area = self.area.as_ref().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a list's area is gone"))?;
        let mut run = area.run()?;
        for item in items {
            run.push(&encode(item))?;
        }
        Ok(run)
    }

    /// Its items from the `start`th on, taken from it as a list of their
    /// own: a run it cuts across, which a place `mark` gave never is, read
    /// back with `decode` first, so none is taken if it cannot be.
    ///
    /// # Errors
    ///
    /// The spill area's, reading that run; or `decode`'s.
    pub fn take_from(&mut self, start: usize, decode: Decode<'_, T>) -> Result<SpillList<T>> {
        let (mut at, mut first) = (0_usize, self.segments.len());
        for (i, segment) in self.segments.iter().enumerate() {
            if at.saturating_add(segment.len()) > start {
                first = i;
                break;
            }
            at = at.saturating_add(segment.len());
        }
        let read = match self.segments.get_mut(first) {
            Some(Segment::Spilled(run, _)) => (at != start).then(|| read_back(run, decode)).transpose()?,
            Some(Segment::Held(..)) | None => None,
        };
        let mut later = self.segments.split_off(first).into_iter();
        let mut tail = SpillList::new(self.space.clone());
        if let Some(cut) = later.next() {
            let mut head = match (cut, read) {
                (Segment::Held(items, _), _) | (Segment::Spilled(..), Some(items)) => items,
                (whole @ Segment::Spilled(..), None) => {
                    tail.segments.push(whole);
                    Vec::new()
                }
            };
            let rest = head.split_off(start.saturating_sub(at).min(head.len()));
            if !head.is_empty() {
                let kept = head.iter().map(ListItem::bytes).sum();
                self.segments.push(Segment::Held(head, kept));
            }
            if !rest.is_empty() {
                let taken = rest.iter().map(ListItem::bytes).sum();
                tail.segments.push(Segment::Held(rest, taken));
            }
        }
        tail.segments.extend(later);
        tail.count = self.count.saturating_sub(start.min(self.count));
        self.count = self.count.min(start);
        self.held = self.segments.iter().map(Segment::held_bytes).sum();
        tail.held = tail.segments.iter().map(Segment::held_bytes).sum();
        Ok(tail)
    }

    /// Its segments' items, each segment's in order, the newest segment
    /// first if `newest`, else the oldest; a run read back whole with
    /// `decode` as its segment is reached, its error in its place.
    pub fn into_segments<'a>(self, newest: bool, decode: Decode<'a, T>) -> impl Iterator<Item = Result<Vec<T>>> + 'a
    where
        T: 'a,
    {
        let mut segments = self.segments;
        if newest {
            segments.reverse();
        }
        segments.into_iter().map(move |segment| read_once(segment, decode))
    }

    /// Its oldest segment's items, taken from it, in order: a run read back
    /// once with `decode`, its pages given back as it is read; none once it
    /// has none. A run that cannot be read back is taken all the same, its
    /// error in its items' place.
    pub fn take_oldest(&mut self, decode: Decode<'_, T>) -> Option<Result<Vec<T>>> {
        if self.segments.is_empty() {
            return None;
        }
        let segment = self.segments.remove(0);
        self.count = self.count.saturating_sub(segment.len());
        self.held = self.held.saturating_sub(segment.held_bytes());
        Some(read_once(segment, decode))
    }

    /// Where its `place`th item is: held, or in a run, by the run's
    /// segment and the item's place in it; none past its last.
    pub fn locate(&self, place: usize) -> Option<Located<'_, T>> {
        let mut at = place;
        for (i, segment) in self.segments.iter().enumerate() {
            if at < segment.len() {
                return Some(match segment {
                    Segment::Held(items, _) => Located::Held(items.get(at)?),
                    Segment::Spilled(..) => Located::Spilled { segment: i, at },
                });
            }
            at = at.saturating_sub(segment.len());
        }
        None
    }

    /// Calls `each` with every item, oldest first, the list kept: runs read
    /// back with `decode`, a segment at a time.
    ///
    /// # Errors
    ///
    /// A run's, `decode`'s, or `each`'s.
    pub fn visit(&mut self, decode: Decode<'_, T>, each: &mut dyn FnMut(&T) -> Result<()>) -> Result<()> {
        for segment in &mut self.segments {
            match segment {
                Segment::Held(items, _) => items.iter().try_for_each(&mut *each)?,
                Segment::Spilled(run, _) => read_back(run, decode)?.iter().try_for_each(&mut *each)?,
            }
        }
        Ok(())
    }

    /// How many segments it has.
    pub fn segments(&self) -> usize {
        self.segments.len()
    }

    /// The items of its `at`th segment, oldest first, copied, the list
    /// kept: a run read back with `decode`; none past its last segment.
    ///
    /// # Errors
    ///
    /// The run's, or `decode`'s.
    pub fn segment(&mut self, at: usize, decode: Decode<'_, T>) -> Result<Vec<T>>
    where
        T: Clone,
    {
        match self.segments.get_mut(at) {
            Some(Segment::Held(items, _)) => Ok(items.clone()),
            Some(Segment::Spilled(run, _)) => read_back(run, decode),
            None => Ok(Vec::new()),
        }
    }

    /// Calls `each` with every item, newest first, the list kept: runs read
    /// back with `decode`, a segment at a time.
    ///
    /// # Errors
    ///
    /// A run's, `decode`'s, or `each`'s.
    pub fn visit_newest(&mut self, decode: Decode<'_, T>, each: &mut dyn FnMut(&T) -> Result<()>) -> Result<()> {
        for segment in self.segments.iter_mut().rev() {
            match segment {
                Segment::Held(items, _) => items.iter().rev().try_for_each(&mut *each)?,
                Segment::Spilled(run, _) => read_back(run, decode)?.iter().rev().try_for_each(&mut *each)?,
            }
        }
        Ok(())
    }

    /// Appends the items of `other` after its own, as they are: held, or in
    /// their runs.
    pub fn append(&mut self, other: SpillList<T>) {
        self.held = self.held.saturating_add(other.held);
        self.count = self.count.saturating_add(other.count);
        self.segments.extend(other.segments);
        // A run holds what it needs of its area; the other's area is kept
        // only if this list has none.
        if self.area.is_none() {
            self.area = other.area;
        }
    }
}

/// Where an item of a list is (`SpillList::locate`).
pub enum Located<'a, T> {
    /// Held in memory.
    Held(&'a T),
    /// In the run of its `segment`th segment, the `at`th item there.
    Spilled {
        /// The segment.
        segment: usize,
        /// Its place in the segment.
        at: usize,
    },
}

/// The items of `segment`, a run's read back once with `decode`.
fn read_once<T>(segment: Segment<T>, decode: Decode<'_, T>) -> Result<Vec<T>> {
    match segment {
        Segment::Held(items, _) => Ok(items),
        Segment::Spilled(run, _) => run.into_records().and_then(|records| records.map(|r| r.and_then(|record| decode(&record))).collect()),
    }
}

/// The items of `run`, read back with `decode`, the run kept.
fn read_back<T>(run: &mut Box<dyn SpillRun>, decode: Decode<'_, T>) -> Result<Vec<T>> {
    run.records()?.map(|record| record.and_then(|r| decode(&r))).collect()
}

// Unit tests: segments, marks, reading back (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
