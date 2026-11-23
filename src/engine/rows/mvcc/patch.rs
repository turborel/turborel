//! \[DESIGN\] A row's update written as a patch of its newest version
//! (RULES.md D916). An update told the columns it sets holds their values
//! alone. Its write reads the row's newest version, as every write does to
//! check it against what another transaction wrote, makes the new record
//! of that version's bytes with the columns set written again
//! (`record/patched.rs`, D915), and writes it over the version it read,
//! which the tree replaces only if it is still the entry. So the record
//! is made of the version it replaces, never of one another transaction
//! put there since.
//!
//! The version, its place in the undo store, the change noted, the zone
//! and the counts are those of a row written whole, as the record is the
//! same bytes. The row after is given to the caller as its log record
//! holds it, made of the record written (`record/image.rs`). A record that
//! is not made so is written whole, the row read and given the columns
//! set (`Patch::whole`). That is so of a table whose rows are sealed or
//! hold large objects, of a row frozen, and of a record that keeps a
//! value out of line, or would once changed.

use std::sync::Mutex;

use super::versions::give_back;
use super::{Open, Past, conflict};
use crate::engine::rows::framed::Framed;
use crate::engine::rows::version::{FRAMED, Version};
use crate::engine::rows::{RowTable, no_row};
use crate::error::Result;
use crate::storage::{AfterImage, Patch, Row, RowId};
use crate::txn::TxId;

/// What one try at a patch did.
enum Tried {
    /// It replaced this version.
    Wrote(Vec<u8>),
    /// The entry it read was gone or changed by then.
    Moved,
    /// The record is not made of the one it replaces.
    Refused,
}

impl RowTable {
    /// Row `id` given by `tx` the values `row` holds in the columns
    /// `patch` sets, every other column kept: the row it replaced, its
    /// columns `read` alone decoded, and the row after as `patch` takes
    /// its image.
    ///
    /// # Errors
    ///
    /// As `write_over_reading`.
    pub(super) fn replace_patching(&self, tx: TxId, id: RowId, row: &Row, (mut patch, read): (Patch<'_>, &[usize])) -> Result<Row> {
        if let Some(old) = self.write_patched(tx, id, (row, patch.set), (read, &mut *patch.image))? {
            return Ok(old);
        }
        let whole = patch.whole(self, tx, id, row)?;
        self.write_over_reading(tx, id, &whole, (Past::Snapshot, read))?.map_or_else(|| no_row(id), Ok)
    }

    /// Whether a record of the table may be made of the one it replaces:
    /// stored as it is, through no encoding, and naming no large object,
    /// whose owners a write finds in the row.
    fn patches(&self) -> bool {
        self.sealing.passes_none() && self.lobs.columns().is_empty()
    }

    /// `replace_patching`'s write, if the record is made of the one it
    /// replaces: none, and nothing written, if it is not. An entry gone or
    /// changed as it was written is tried once more, as `write_version`
    /// tries it.
    fn write_patched(&self, tx: TxId, id: RowId, new: (&Row, &[usize]), (read, image): (&[usize], &mut AfterImage)) -> Result<Option<Row>> {
        if !self.patches() {
            return Ok(None);
        }
        let opened = self.open_of(tx)?;
        let open: &Mutex<Open> = &opened;
        let mut framed = Framed::of(Framed::buffer());
        let replaced = match self.try_patch((tx, open), id, new, &mut framed)? {
            Tried::Wrote(replaced) => replaced,
            Tried::Refused => return Ok(None),
            Tried::Moved => match self.try_patch((tx, open), id, new, &mut framed)? {
                Tried::Wrote(replaced) => replaced,
                Tried::Refused => return Ok(None),
                Tried::Moved => return Err(conflict()),
            },
        };
        let record = framed.get(FRAMED..).unwrap_or_default();
        let old = self.replaced(id, Some(&replaced), Some(read));
        let widened = self.widen_patched(id, record, &replaced);
        give_back(replaced);
        self.counted_change(tx);
        // The row after, of the record written: the caller holds of it
        // only the values it set.
        let imaged = self.format.image(record, new.1, image);
        widened.and(imaged).and(old).map(|old| old.map(|(was, _)| was))
    }

    /// One try: row `id`'s newest version read, the record made of it in
    /// `framed`, behind the room for its version's header, and written
    /// over it. A deletion has no record to make one of, and its write is
    /// refused as any write over it is.
    fn try_patch(&self, (tx, open): (TxId, &Mutex<Open>), id: RowId, new: (&Row, &[usize]), framed: &mut Framed) -> Result<Tried> {
        let Some((newest, found)) = self.newest_or_frozen(id)? else { return no_row(id) };
        let descended = match found {
            Ok(descended) => descended,
            // A frozen row's version was made for this write: it is
            // written whole, its values out of line made again.
            Err(pages) => {
                self.free(&pages);
                return Ok(Tried::Refused);
            }
        };
        let version = Version::parse(&newest)?;
        framed.clear();
        framed.resize(FRAMED, 0);
        if !version.deleted && !self.format.patched(version.record, new, Some(self.spill), framed)? {
            give_back(newest);
            return Ok(Tried::Refused);
        }
        let written = self.write_found((tx, open), id, (newest, Ok(descended)), (Some(framed.as_mut_slice()), Past::Snapshot))?;
        Ok(written.map_or(Tried::Moved, Tried::Wrote))
    }
}

// Unit tests: a patch leaves what a whole write leaves (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests;
