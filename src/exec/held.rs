//! \[DESIGN\] Rows a statement holds past the operator that made them
//! (RULES.md D38): a cursor's rows read at OPEN, or before its transaction
//! changes a table it reads, and a SCROLL cursor's as it reads them; the
//! object rows of a searched UPDATE or DELETE and the candidate rows of an
//! INSERT, each chosen before any is changed; an updatable cursor's rows,
//! in its order; what a data-changing statement did, and the checks of its
//! constraints (`db/integrity/held.rs`). Held up to MEMORY.WORK_BYTES, as an operator's are, then written
//! to runs of a spill area: a list that spills (`spill/list.rs`), read back
//! in order, a run's pages given back as it is read, or by place, a
//! segment at a time. Split from exec.rs (RULES.md R-SIZE-2).

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::spill::{ListItem, ListSpace, Located, SpillList, row_bytes};
use crate::storage::{Row, RowId};
use crate::value::Value;
use crate::value::codec::{put_row, row};
use crate::value::types::TypeRegistry;

/// What a list of `HeldRows` holds: an item and its record.
pub trait Holdable: ListItem + Clone + Send + Sized + 'static {
    /// It, as a record.
    fn record(&self) -> Vec<u8>;
    /// The item of `record`, its values of types beyond the standard found
    /// among `types`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `record` is not one.
    fn read(record: &[u8], types: &TypeRegistry) -> Result<Self>;
}

impl ListItem for Vec<Value> {
    fn bytes(&self) -> usize {
        row_bytes(self)
    }
}

impl Holdable for Row {
    fn record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_row(&mut out, self);
        out
    }
    fn read(record: &[u8], types: &TypeRegistry) -> Result<Row> {
        let mut r = Reader::new(record, "held row").with_types(types);
        let found = row(&mut r)?;
        r.finish()?;
        Ok(found)
    }
}

impl ListItem for (RowId, Row) {
    fn bytes(&self) -> usize {
        row_bytes(&self.1).saturating_add(size_of::<RowId>())
    }
}

impl Holdable for (RowId, Row) {
    fn record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.put_u64(self.0);
        put_row(&mut out, &self.1);
        out
    }
    fn read(record: &[u8], types: &TypeRegistry) -> Result<(RowId, Row)> {
        let mut r = Reader::new(record, "held row").with_types(types);
        let id = r.u64()?;
        let found = row(&mut r)?;
        r.finish()?;
        Ok((id, found))
    }
}

impl ListItem for (RowId, Row, Row) {
    fn bytes(&self) -> usize {
        row_bytes(&self.1).saturating_add(row_bytes(&self.2)).saturating_add(size_of::<RowId>())
    }
}

/// A row's id, its values, and its new values: an UPDATE's change.
impl Holdable for (RowId, Row, Row) {
    fn record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.put_u64(self.0);
        put_row(&mut out, &self.1);
        put_row(&mut out, &self.2);
        out
    }
    fn read(record: &[u8], types: &TypeRegistry) -> Result<(RowId, Row, Row)> {
        let mut r = Reader::new(record, "held change").with_types(types);
        let id = r.u64()?;
        let (old, new) = (row(&mut r)?, row(&mut r)?);
        r.finish()?;
        Ok((id, old, new))
    }
}

impl ListItem for RowId {
    fn bytes(&self) -> usize {
        size_of::<RowId>()
    }
}

impl Holdable for RowId {
    fn record(&self) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
    fn read(record: &[u8], _types: &TypeRegistry) -> Result<RowId> {
        let mut r = Reader::new(record, "held row id");
        let id = r.u64()?;
        r.finish()?;
        Ok(id)
    }
}

/// \[DESIGN\] Rows held, in the order added: up to a work area's bytes in
/// memory, the rest in runs of a spill area.
pub struct HeldRows<T: Holdable> {
    list: SpillList<T>,
    types: TypeRegistry,
    /// The items of the spilled segment read last by place, and which it
    /// is: a segment written is never changed.
    window: Option<(usize, Vec<T>)>,
}

impl<T: Holdable> HeldRows<T> {
    /// None yet, held in `space`.
    pub fn new(space: ListSpace) -> HeldRows<T> {
        let types = space.types.clone();
        HeldRows { list: SpillList::new(space), types, window: None }
    }

    /// Adds `item` at the end.
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved.
    pub fn push(&mut self, item: T) -> Result<()> {
        self.list.push(item, &mut |t: &T| t.record())
    }

    /// How many it holds.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Whether some are written to a spill area: the first are, once any
    /// is.
    pub fn spilled(&self) -> bool {
        matches!(self.list.locate(0), Some(Located::Spilled { .. }))
    }

    /// The `place`th, none past the last: from memory, or read back with
    /// the rest of its segment, which is kept for the next.
    ///
    /// # Errors
    ///
    /// The spill area's, reading the segment.
    pub fn get(&mut self, place: usize) -> Result<Option<T>> {
        let (segment, at) = match self.list.locate(place) {
            None => return Ok(None),
            Some(Located::Held(item)) => return Ok(Some(item.clone())),
            Some(Located::Spilled { segment, at }) => (segment, at),
        };
        if self.window.as_ref().is_none_or(|(read, _)| *read != segment) {
            let types = self.types.clone();
            let items = self.list.segment(segment, &move |record: &[u8]| T::read(record, &types))?;
            self.window = Some((segment, items));
        }
        Ok(self.window.as_ref().and_then(|(_, items)| items.get(at)).cloned())
    }

    /// Appends the items of `other` after its own, as they are: held, or in
    /// their runs.
    pub fn append(&mut self, other: HeldRows<T>) {
        self.list.append(other.list);
    }

    /// Calls `each` with every one, in order, kept: a run read back a
    /// segment at a time.
    ///
    /// # Errors
    ///
    /// The spill area's, or `each`'s.
    pub fn visit(&mut self, each: &mut dyn FnMut(&T) -> Result<()>) -> Result<()> {
        let types = self.types.clone();
        self.list.visit(&move |record: &[u8]| T::read(record, &types), each)
    }
}

impl<T: Holdable> IntoIterator for HeldRows<T> {
    type Item = Result<T>;
    type IntoIter = HeldIter<T>;

    fn into_iter(self) -> HeldIter<T> {
        HeldIter { list: self.list, types: self.types, read: Vec::new().into_iter() }
    }
}

/// Rows held, given back in order, each segment's pages given back as it is
/// read; a segment that cannot be read back, its error in its place.
pub struct HeldIter<T: Holdable> {
    list: SpillList<T>,
    types: TypeRegistry,
    /// The rest of the segment being read.
    read: std::vec::IntoIter<T>,
}

impl<T: Holdable> Iterator for HeldIter<T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        loop {
            if let Some(item) = self.read.next() {
                return Some(Ok(item));
            }
            let types = self.types.clone();
            match self.list.take_oldest(&move |record: &[u8]| T::read(record, &types))? {
                Ok(items) => self.read = items.into_iter(),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

// Unit tests: rows held, spilled and read back (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
