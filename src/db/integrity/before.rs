//! \[EXTENSION\] A referenced table as it was before a statement [ISO/IEC
//! 9075:1992 §11.8 GR4], for the referential actions of MATCH PARTIAL
//! (`integrity/actions.rs`): its rows, held as a list that spills, made
//! once for each table; and, for each set of its columns that a row's
//! referencing values not null are compared with [GR3b i], a lookup of its
//! rows by their values there, made by one pass over them when first
//! needed, so the rows each row matched are found without reading the
//! table again for each. Split from `integrity/actions.rs` (RULES.md
//! R-SIZE-2).
//!
//! The lookups are a map that spills (`spill/map.rs`, RULES.md D38). Its
//! keys are a lookup's columns' values as an index encodes them
//! (`engine/key.rs`), the same bytes for values equal by comparison: under
//! each, the place of its last row; under each place, its row's id and the
//! place of the row of the key before it. A value not exactly of its
//! column's type has no such bytes: the rows are then compared with the
//! values one by one.
//!
//! ```text
//! head: 0 (u8), the lookup's number u32, the key -> its last row's place u64
//! row:  1 (u8), its place u64 -> its id u64, the place before u64 (0: none)
//! ```

use std::ops::ControlFlow;

use super::Effect;
use super::references::partially;
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::db::{State, api};
use crate::engine::key::anchor;
use crate::error::Result;
use crate::exec::HeldRows;
use crate::spill::ListSpace;
use crate::spill::map::SpillMap;
use crate::storage::{Row, RowId, project};
use crate::txn::Txn;
use crate::value::{DataType, Value};

/// The kinds of a key of the lookups' map (above).
const HEAD: u8 = 0;
const PLACE: u8 = 1;
/// The place of no row: places count from 1.
const NONE: u64 = 0;

#[cfg(test)]
std::thread_local! {
    /// How many rows of tables as they were before a statement this thread
    /// has read: so a test sees the reads a statement's actions make.
    pub(super) static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts a row read, for tests.
fn read_one() {
    #[cfg(test)]
    READS.with(|n| n.set(n.get().saturating_add(1)));
}

/// \[DESIGN\] A table as it was before a statement, and its lookups.
pub(super) struct Before {
    /// The table.
    pub(super) table: QualifiedName,
    rows: HeldRows<(RowId, Row)>,
    /// Its columns' types.
    types: Vec<DataType>,
    /// The columns of each lookup made, by its number, and whether it
    /// holds every row with no null there.
    lookups: Vec<(Vec<usize>, bool)>,
    map: SpillMap,
    /// The place of the last row put in a lookup.
    places: u64,
    space: ListSpace,
}

impl Before {
    /// [§11.8 GR3b i, GR4] The rows a row whose referencing values are
    /// `values`, of the referenced columns `referenced`, matched: whose
    /// values there equal those not null, one at least.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn matched(&mut self, referenced: &[usize], values: &[Value]) -> Result<HeldRows<RowId>> {
        let mut out = HeldRows::new(self.space.clone());
        let (columns, wanted): (Vec<usize>, Vec<&Value>) = referenced.iter().zip(values).filter(|(_, v)| !v.is_null()).unzip();
        if columns.is_empty() {
            return Ok(out);
        }
        let key = key_of(&self.types, &columns, wanted.into_iter());
        let lookup = match key {
            Some(_) => self.lookup(&columns)?,
            None => None,
        };
        match (lookup, key) {
            (Some(number), Some(key)) => self.chain(number, &key, &mut out)?,
            (None, _) | (_, None) => self.rows.visit(&mut |(id, row)| {
                read_one();
                if project(row, referenced).is_some_and(|k| partially(values, &k)) { out.push(*id) } else { Ok(()) }
            })?,
        }
        Ok(out)
    }

    /// The number of the lookup on `columns`, made by a pass over the rows
    /// if there is none yet: none if a row's value there is not exactly of
    /// its column's type.
    fn lookup(&mut self, columns: &[usize]) -> Result<Option<u32>> {
        if let Some(at) = self.lookups.iter().position(|(c, _)| c == columns) {
            return Ok(self.lookups.get(at).and_then(|(_, whole)| whole.then(|| u32::try_from(at).ok())).flatten());
        }
        let number = u32::try_from(self.lookups.len()).map_err(|_| api("[DESIGN] too many lookups of a table before a statement"))?;
        let (types, map, places) = (&self.types, &mut self.map, &mut self.places);
        let mut whole = true;
        self.rows.visit(&mut |(id, row)| {
            read_one();
            let values: Option<Vec<&Value>> = columns.iter().map(|&c| row.get(c)).collect();
            let Some(values) = values.filter(|v| !v.iter().any(|x| x.is_null())) else { return Ok(()) };
            let Some(key) = key_of(types, columns, values.into_iter()) else {
                whole = false;
                return Ok(());
            };
            let head = head_key(number, &key);
            let before = map.get(&head)?.map_or(Ok(NONE), |at| Reader::new(&at, "lookup").u64())?;
            *places = places.saturating_add(1);
            let mut entry = Vec::new();
            entry.put_u64(*id);
            entry.put_u64(before);
            map.insert(&place_key(*places), &entry)?;
            map.insert(&head, &places.to_le_bytes())
        })?;
        self.lookups.push((columns.to_vec(), whole));
        Ok(whole.then_some(number))
    }

    /// Adds to `out` the rows lookup `number` holds under `key`.
    fn chain(&self, number: u32, key: &[u8], out: &mut HeldRows<RowId>) -> Result<()> {
        let mut at = match self.map.get(&head_key(number, key))? {
            Some(head) => Reader::new(&head, "lookup").u64()?,
            None => NONE,
        };
        while at != NONE {
            let entry = self.map.get(&place_key(at))?.ok_or_else(|| api("[DESIGN] a lookup's row is missing"))?;
            let mut r = Reader::new(&entry, "lookup");
            out.push(r.u64()?)?;
            at = r.u64()?;
        }
        Ok(())
    }
}

/// The bytes of `values`, of `columns` of a table whose columns' types are
/// `types`, as an index encodes them; none if one is not exactly of its
/// column's type, or null.
fn key_of<'v>(types: &[DataType], columns: &[usize], values: impl Iterator<Item = &'v Value>) -> Option<Vec<u8>> {
    let mut key = Vec::new();
    for (c, v) in columns.iter().zip(values) {
        let at = anchor(v, *types.get(*c)?)?;
        if !at.exact {
            return None;
        }
        key.extend(at.key);
    }
    Some(key)
}

/// The map's key of the last row under `key` in lookup `number`.
fn head_key(number: u32, key: &[u8]) -> Vec<u8> {
    let mut out = vec![HEAD];
    out.put_u32(number);
    out.extend_from_slice(key);
    out
}

/// The map's key of the row at `place`.
fn place_key(place: u64) -> Vec<u8> {
    let mut out = vec![PLACE];
    out.put_u64(place);
    out
}

impl State {
    /// [§11.8 GR4] `table` as it was before the statement that did
    /// `done`, its rows held as a list that spills, in `space`: as they
    /// are, but for the rows it changed, each as it was first, none of one
    /// it inserted. The ids of the rows it changed are kept in a map that
    /// spills.
    ///
    /// # Errors
    ///
    /// The table's; the spill area's.
    pub(super) fn before_statement(
        &self,
        tx: &Txn,
        table: &QualifiedName,
        done: &mut HeldRows<Effect>,
        space: &ListSpace,
    ) -> Result<Before> {
        let mut changed = SpillMap::new(space.clone());
        done.visit(&mut |effect| {
            let (t, id, _) = effect_row(effect);
            if t == table { changed.insert(&id.to_le_bytes(), &[]) } else { Ok(()) }
        })?;
        let mut rows = HeldRows::new(space.clone());
        self.each_row_where(tx, table, &|_| true, &mut |id, row| {
            if !changed.contains(&id.to_le_bytes())? {
                rows.push((id, row))?;
            }
            Ok(ControlFlow::Continue(()))
        })?;
        done.visit(&mut |effect| {
            let (t, id, was) = effect_row(effect);
            if t != table {
                return Ok(());
            }
            // Its first effect, taken from those to come: what it was.
            let first = changed.remove(&id.to_le_bytes())?;
            match (first, was) {
                (true, Some(old)) => rows.push((id, old.clone())),
                (true, None) | (false, _) => Ok(()),
            }
        })?;
        let types = self.catalog.table(table).map(|t| t.columns.iter().map(|c| c.data_type).collect()).unwrap_or_default();
        let (map, space) = (SpillMap::new(space.clone()), space.clone());
        Ok(Before { table: table.clone(), rows, types, lookups: Vec::new(), map, places: NONE, space })
    }
}

/// The table, row and old values of what `effect` did: none of a row
/// inserted.
fn effect_row(effect: &Effect) -> (&QualifiedName, RowId, Option<&Row>) {
    match effect {
        Effect::Inserted { table, id } => (table, *id, None),
        Effect::Updated { table, id, old, .. } | Effect::Deleted { table, id, old } => (table, *id, Some(old)),
    }
}

// Unit tests: the rows read of a table as it was, linear in its rows and
// in those matching (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
