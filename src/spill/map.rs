//! \[DESIGN\] A map that spills (RULES.md D38): byte keys, each to one byte
//! value, held in memory until they pass a number of bytes, then all moved
//! to a table of a spill area made then, where the rest go too. As an
//! operator's counted keys do (`exec/work.rs`), for what is kept by key
//! beside a list that spills: the values a statement's referential actions
//! gave each row, and which referenced rows each row is no longer
//! associated with (`db/integrity/actions.rs`). A set is a map whose
//! values are empty.

use std::collections::HashMap;

use super::{Area, ListSpace, SpillArea, SpillTable};
use crate::error::{Code, Result, SqlError};

/// The bytes an entry held in memory takes beyond its key's and value's:
/// its place in the map, near enough.
const ENTRY_BYTES: usize = 48;

/// \[DESIGN\] The map.
pub struct SpillMap {
    space: ListSpace,
    held: HashMap<Vec<u8>, Vec<u8>>,
    /// The bytes of what is held.
    bytes: usize,
    /// Its table once it spills, and the area of it, dropped after it.
    table: Option<Box<dyn SpillTable>>,
    area: Option<Box<dyn SpillArea>>,
}

impl SpillMap {
    /// An empty map, spilling in `space`: past its limit, if it has work
    /// areas.
    pub fn new(space: ListSpace) -> SpillMap {
        SpillMap { space, held: HashMap::new(), bytes: 0, table: None, area: None }
    }

    /// Whether its entries are in a spill area.
    #[cfg(test)]
    pub fn spilled(&self) -> bool {
        self.table.is_some()
    }

    /// The value of `key`, if it has one.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        match &self.table {
            Some(table) => Ok(table.get(key)?.into_iter().next()),
            None => Ok(self.held.get(key).cloned()),
        }
    }

    /// Whether `key` has a value.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub fn contains(&self, key: &[u8]) -> Result<bool> {
        match &self.table {
            Some(table) => Ok(!table.get(key)?.is_empty()),
            None => Ok(self.held.contains_key(key)),
        }
    }

    /// Gives `key` the value `value`, in place of any it had; written to
    /// the spill area, with every other, once what is held passes the
    /// space's limit.
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved.
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if let Some(table) = &mut self.table {
            if let Some(was) = table.get(key)?.into_iter().next() {
                table.remove(key, &was)?;
            }
            return table.insert(key, value);
        }
        let entry = key.len().saturating_add(value.len()).saturating_add(ENTRY_BYTES);
        match self.held.insert(key.to_vec(), value.to_vec()) {
            Some(was) => self.bytes = self.bytes.saturating_add(value.len()).saturating_sub(was.len()),
            None => self.bytes = self.bytes.saturating_add(entry),
        }
        if self.bytes > self.space.limit && self.space.areas.is_some() { self.spill() } else { Ok(()) }
    }

    /// Takes away `key`'s value; whether it had one.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub fn remove(&mut self, key: &[u8]) -> Result<bool> {
        if let Some(table) = &mut self.table {
            let Some(was) = table.get(key)?.into_iter().next() else { return Ok(false) };
            return table.remove(key, &was);
        }
        let Some(was) = self.held.remove(key) else { return Ok(false) };
        self.bytes = self.bytes.saturating_sub(key.len().saturating_add(was.len()).saturating_add(ENTRY_BYTES));
        Ok(true)
    }

    /// Moves what it holds to a table of a spill area of its own.
    fn spill(&mut self) -> Result<()> {
        let areas = self.space.areas.as_ref().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a map that spills has no work areas"))?;
        let area = areas.area(Area::new(self.space.limit, self.space.batch))?;
        let mut table = area.table()?;
        for (key, value) in &self.held {
            table.insert(key, value)?;
        }
        self.held = HashMap::new();
        self.bytes = 0;
        self.table = Some(table);
        self.area = Some(area);
        Ok(())
    }
}

// Unit tests: held, spilled, read back (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
