//! \[DESIGN\] Rows a statement holds past the operator that made them
//! (RULES.md D38): a cursor's rows read at OPEN, or before its transaction
//! changes a table it reads, and a SCROLL cursor's as it reads them; the
//! object rows of a searched UPDATE or DELETE and the candidate rows of an
//! INSERT, each chosen before any is changed; an updatable cursor's rows, in
//! its order; what a data-changing statement did, and the checks of its
//! constraints (`db/integrity/held.rs`). Held up to MEMORY.WORK_BYTES, as an
//! operator's are, then written to runs of a spill area: a list that spills
//! (`spill/list.rs`), read back in order, a run's pages given back as it is
//! read, or by place, a segment at a time. A large object or an XML value
//! a spilled row holds kept apart is named by what keeps it, which the list
//! holds while it is (`spill/named.rs`, D79).

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::spill::{ListItem, ListSpace, Located, SpillList, SpilledLobs, record_room, row_bytes};
use crate::storage::{Row, RowId};
use crate::value::Value;
use crate::value::types::TypeRegistry;

/// What a list of `HeldRows` holds: an item and its record, its large
/// objects and XML values kept apart named by what keeps them, which the
/// list holds.
pub trait Holdable: ListItem + Clone + Send + Sized + 'static {
    /// It, as a record, what it names held by `lobs`.
    fn record(&self, lobs: &SpilledLobs) -> Vec<u8>;
    /// The item of `record`, its values of types beyond the standard found
    /// among `types`, and what it names among `lobs`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `record` is not one.
    fn read(record: &[u8], types: &TypeRegistry, lobs: &SpilledLobs) -> Result<Self>;
}

impl ListItem for Vec<Value> {
    fn bytes(&self) -> usize {
        row_bytes(self)
    }
}

impl Holdable for Row {
    fn record(&self, lobs: &SpilledLobs) -> Vec<u8> {
        lobs.record(self)
    }
    fn read(record: &[u8], types: &TypeRegistry, lobs: &SpilledLobs) -> Result<Row> {
        lobs.record_row(record, types)
    }
}

impl ListItem for (RowId, Row) {
    fn bytes(&self) -> usize {
        row_bytes(&self.1).saturating_add(size_of::<RowId>())
    }
}

impl Holdable for (RowId, Row) {
    fn record(&self, lobs: &SpilledLobs) -> Vec<u8> {
        let mut out = Vec::with_capacity(record_room(size_of::<RowId>().saturating_add(1), self.1.len()));
        out.put_u64(self.0);
        lobs.put_row(&mut out, &self.1);
        out
    }
    fn read(record: &[u8], types: &TypeRegistry, lobs: &SpilledLobs) -> Result<(RowId, Row)> {
        let mut r = Reader::new(record, "held row").with_types(types);
        let id = r.u64()?;
        let found = lobs.row(&mut r)?;
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
    fn record(&self, lobs: &SpilledLobs) -> Vec<u8> {
        let mut out = Vec::new();
        out.put_u64(self.0);
        lobs.put_row(&mut out, &self.1);
        lobs.put_row(&mut out, &self.2);
        out
    }
    fn read(record: &[u8], types: &TypeRegistry, lobs: &SpilledLobs) -> Result<(RowId, Row, Row)> {
        let mut r = Reader::new(record, "held change").with_types(types);
        let id = r.u64()?;
        let (old, new) = (lobs.row(&mut r)?, lobs.row(&mut r)?);
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
    fn record(&self, _lobs: &SpilledLobs) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
    fn read(record: &[u8], _types: &TypeRegistry, _lobs: &SpilledLobs) -> Result<RowId> {
        let mut r = Reader::new(record, "held row id");
        let id = r.u64()?;
        r.finish()?;
        Ok(id)
    }
}

/// \[DESIGN\] Rows held, in the order added: up to a work area's bytes in
/// memory, the rest in runs of a spill area, with the sources of what they
/// name.
pub struct HeldRows<T: Holdable> {
    list: SpillList<T>,
    types: TypeRegistry,
    lobs: SpilledLobs,
    /// The items of the spilled segment read last by place, and which it
    /// is: a segment written is never changed.
    window: Option<(usize, Vec<T>)>,
}

impl<T: Holdable> HeldRows<T> {
    /// None yet, held in `space`.
    pub fn new(space: ListSpace) -> HeldRows<T> {
        let types = space.types.clone();
        HeldRows { list: SpillList::new(space), types, lobs: SpilledLobs::new(), window: None }
    }

    /// The reader of its records.
    fn decode(&self) -> impl Fn(&[u8]) -> Result<T> + Sync + use<T> {
        let (types, lobs) = (self.types.clone(), self.lobs.clone());
        move |record: &[u8]| T::read(record, &types, &lobs)
    }

    /// Adds `item` at the end.
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved.
    pub fn push(&mut self, item: T) -> Result<()> {
        let lobs = &self.lobs;
        self.list.push(item, &mut |t: &T| t.record(lobs))
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
            let decode = self.decode();
            let items = self.list.segment(segment, &decode)?;
            self.window = Some((segment, items));
        }
        Ok(self.window.as_ref().and_then(|(_, items)| items.get(at)).cloned())
    }

    /// Where its `place`th is: held, or in which spilled segment and where
    /// there; none past the last. A reader of its own segments by place
    /// reads them so (`exec/window/reader.rs`).
    pub fn locate(&self, place: usize) -> Option<Located<'_, T>> {
        self.list.locate(place)
    }

    /// The items of its `segment`th segment, read back, the list kept.
    ///
    /// # Errors
    ///
    /// The spill area's, reading the segment.
    pub fn segment(&mut self, segment: usize) -> Result<Vec<T>> {
        let decode = self.decode();
        self.list.segment(segment, &decode)
    }

    /// Appends the items of `other` after its own, as they are: held, or in
    /// their runs, the sources they name held with its own.
    pub fn append(&mut self, other: HeldRows<T>) {
        self.lobs.absorb(&other.lobs);
        self.list.append(other.list);
    }

    /// Calls `each` with every one, in order, kept: a run read back a
    /// segment at a time.
    ///
    /// # Errors
    ///
    /// The spill area's, or `each`'s.
    pub fn visit(&mut self, each: &mut dyn FnMut(&T) -> Result<()>) -> Result<()> {
        let decode = self.decode();
        self.list.visit(&decode, each)
    }
}

impl<T: Holdable> IntoIterator for HeldRows<T> {
    type Item = Result<T>;
    type IntoIter = HeldIter<T>;

    fn into_iter(self) -> HeldIter<T> {
        HeldIter { list: self.list, types: self.types, lobs: self.lobs, read: Vec::new().into_iter() }
    }
}

/// Rows held, given back in order, each segment's pages given back as it is
/// read; a segment that cannot be read back, its error in its place.
pub struct HeldIter<T: Holdable> {
    list: SpillList<T>,
    types: TypeRegistry,
    /// The sources what it holds names, held while it is.
    lobs: SpilledLobs,
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
            let (types, lobs) = (self.types.clone(), self.lobs.clone());
            match self.list.take_oldest(&move |record: &[u8]| T::read(record, &types, &lobs))? {
                Ok(items) => self.read = items.into_iter(),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

// Unit tests: rows held, spilled and read back (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
