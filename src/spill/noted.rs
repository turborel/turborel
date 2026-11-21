//! \[EXTENSION\] A spill area made for an operator EXPLAIN ANALYZE counts
//! (RULES.md D620): the area its provider made, its tables and runs each
//! noted as made, the bytes of each record written to them noted, and a
//! pass noted each time it reads back what it wrote after writing, so
//! partitions written then each read are one pass, and each level of a
//! merge is another. An area of a statement not analyzed is the provider's
//! own, unwrapped (`WorkAreas::area`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Batches, Records, SpillArea, SpillRun, SpillTable};
use crate::error::Result;
use crate::noted::{self, Noted, Notes};

/// What an area and its tables and runs note to.
struct Noting {
    notes: Arc<Notes>,
    /// Whether it wrote since it last read.
    writing: AtomicBool,
}

impl Noting {
    /// Notes `bytes` bytes written.
    fn wrote(&self, bytes: usize) {
        self.writing.store(true, Ordering::Relaxed);
        noted::add(Some(&self.notes), Noted::Spilled, bytes);
    }

    /// Notes a read: a pass if it wrote since it last read.
    fn read(&self) {
        if self.writing.swap(false, Ordering::Relaxed) {
            self.notes.add(Noted::Passes, 1);
        }
    }
}

/// `area`, noted to `notes`.
pub(super) fn noted(area: Box<dyn SpillArea>, notes: Arc<Notes>) -> Box<dyn SpillArea> {
    Box::new(NotedArea { area, noting: Arc::new(Noting { notes, writing: AtomicBool::new(false) }) })
}

/// An area noted.
struct NotedArea {
    area: Box<dyn SpillArea>,
    noting: Arc<Noting>,
}

impl SpillArea for NotedArea {
    fn table(&self) -> Result<Box<dyn SpillTable>> {
        let table = self.area.table()?;
        self.noting.notes.add(Noted::Partitions, 1);
        Ok(Box::new(NotedTable { table, noting: Arc::clone(&self.noting) }))
    }

    fn run(&self) -> Result<Box<dyn SpillRun>> {
        let run = self.area.run()?;
        self.noting.notes.add(Noted::Partitions, 1);
        Ok(Box::new(NotedRun { run, noting: Arc::clone(&self.noting) }))
    }
}

/// A table of an area noted.
struct NotedTable {
    table: Box<dyn SpillTable>,
    noting: Arc<Noting>,
}

impl SpillTable for NotedTable {
    fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.noting.wrote(key.len().saturating_add(value.len()));
        self.table.insert(key, value)
    }

    fn get(&self, key: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.noting.read();
        self.table.get(key)
    }

    fn values(&self, key: &[u8]) -> Result<Records> {
        self.noting.read();
        self.table.values(key)
    }

    fn remove(&mut self, key: &[u8], value: &[u8]) -> Result<bool> {
        self.table.remove(key, value)
    }
}

/// A run of an area noted.
struct NotedRun {
    run: Box<dyn SpillRun>,
    noting: Arc<Noting>,
}

impl SpillRun for NotedRun {
    fn push(&mut self, record: &[u8]) -> Result<()> {
        self.noting.wrote(record.len());
        self.run.push(record)
    }

    fn records(&mut self) -> Result<Records> {
        self.noting.read();
        self.run.records()
    }

    fn into_records(self: Box<Self>) -> Result<Records> {
        self.noting.read();
        self.run.into_records()
    }

    fn into_batches(self: Box<Self>) -> Result<Batches> {
        self.noting.read();
        self.run.into_batches()
    }
}

// Unit tests: what an area notes (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::noted::NotesRead;
    use crate::spill::Area;
    use crate::spill::testing::areas;

    /// Verifies: [EXTENSION] RULES.md D620: an area made within an
    /// operator's notes notes each table and run made, the bytes written to
    /// them, and a pass each time it reads after writing; one made with no
    /// notes notes nothing.
    #[test]
    fn an_area_notes_what_it_spills_and_its_passes() {
        let notes = Arc::new(Notes::default());
        let work = areas(None, 1 << 20);
        let area = noted::within(Some(Arc::clone(&notes)), || work.area(Area::new(1 << 16, 4096))).unwrap();
        let (mut a, mut b) = (area.run().unwrap(), area.run().unwrap());
        a.push(b"abc").unwrap();
        b.push(b"de").unwrap();
        assert_eq!(a.records().unwrap().count(), 1);
        assert_eq!(b.into_records().unwrap().count(), 1, "read on: the same pass");
        let mut table = area.table().unwrap();
        table.insert(b"k", b"vv").unwrap();
        assert_eq!(table.get(b"k").unwrap().len(), 1);
        assert_eq!(table.values(b"k").unwrap().count(), 1);
        assert!(table.remove(b"k", b"vv").unwrap());
        let read = notes.read();
        assert_eq!(read, NotesRead { spilled: 8, partitions: 3, passes: 2, ..NotesRead::default() });
        let plain = work.area(Area::new(1 << 16, 4096)).unwrap();
        plain.run().unwrap().push(b"x").unwrap();
        assert_eq!(notes.read(), read, "no notes, nothing noted");
    }
}
