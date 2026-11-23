//! Values of a row-store table's records (RULES.md D31): a row encoded,
//! its long values kept out of line on pages of their own, each page
//! checked against its row and column as it is read, and freed with its
//! row; and a record decoded, all its columns or those asked for.

use super::gathered::Gathering;
use super::version::FRAMED;
use super::{KEY, OWNER, RowTable, corrupt};
use crate::disk::pager::PageId;
use crate::engine::record::{Reference, Spill};
use crate::error::{Code, Result, SqlError};
use crate::storage::{Row, RowId};
use crate::value::Value;
use crate::value::lob::Part;

impl RowTable {
    /// The record of row `id`, holding `row`, and the pages of its values
    /// out of line; on failure, none is left allocated.
    pub(in crate::engine::rows) fn encode(&self, id: RowId, row: &Row) -> Result<(Vec<u8>, Vec<PageId>)> {
        self.encode_after(id, row, 0)
    }

    /// As `encode`, the record behind `FRAMED` bytes kept for its version's
    /// header, so the version is written in the same buffer (RULES.md
    /// D846).
    pub(in crate::engine::rows) fn encode_framed(&self, id: RowId, row: &Row) -> Result<(Vec<u8>, Vec<PageId>)> {
        self.encode_after(id, row, FRAMED)
    }

    /// As `encode`, the record behind `room` bytes.
    fn encode_after(&self, id: RowId, row: &Row, room: usize) -> Result<(Vec<u8>, Vec<PageId>)> {
        self.check_lobs(row)?;
        let mut pages = Vec::new();
        // Sized for the record written in line, as most are, so the room
        // and the record are one allocation, not one grown into another.
        let mut record = Vec::with_capacity(room.saturating_add(self.format.in_line_room(row)));
        record.resize(room, 0);
        // A record sealed takes more: room is kept for it.
        let spill = Spill { most: self.spill.most.saturating_sub(self.sealing.overhead()), ..self.spill };
        let encoded = self.format.encode_spilling(row, spill, &mut record, |column, bytes| {
            let page = self.put_value(id, column, bytes)?;
            pages.push(page);
            Ok(page.0)
        });
        match encoded.and_then(|()| self.sealing.record_after(id, record, room)) {
            Ok(stored) => Ok((stored, pages)),
            Err(e) => {
                self.free(&pages);
                Err(e)
            }
        }
    }

    /// Refuses a large object of `row` the table does not keep: one held
    /// in memory, one it keeps, and one named by reference, as the log
    /// names one, are written (`rows/lobs/owned.rs`).
    fn check_lobs(&self, row: &Row) -> Result<()> {
        for &(c, _) in self.lobs.columns() {
            if let Some(l) = row.get(c).and_then(Value::as_lob)
                && l.inline().is_none()
                && !matches!(l.parts(), [Part::Reference(_)])
                && self.lobs.kept_here(l).is_none()
            {
                return Err(SqlError::new(
                    Code::Api,
                    format!("[DESIGN] column {c}'s large object is not kept by the table: copy it first"),
                ));
            }
        }
        Ok(())
    }

    /// Keeps `bytes`, column `column`'s value of row `id`, in a new page.
    pub(in crate::engine::rows) fn put_value(&self, id: RowId, column: usize, bytes: &[u8]) -> Result<PageId> {
        let column = u32::try_from(column).map_err(|_| SqlError::new(Code::Unsupported, "[DESIGN] a table has too many columns"))?;
        let mut stored = Vec::with_capacity(OWNER.saturating_add(bytes.len()).saturating_add(1));
        stored.extend_from_slice(&id.to_le_bytes());
        stored.extend_from_slice(&column.to_le_bytes());
        self.sealing.value(id, bytes, &mut stored)?;
        let (page, mut content) = self.pool.new_page(self.file)?;
        *content = stored;
        Ok(page)
    }

    /// The value `reference` names, of row `id`: refused if its page is not
    /// that row's column's.
    pub(in crate::engine::rows) fn get_value(&self, id: RowId, reference: Reference) -> Result<Vec<u8>> {
        let page = self.pool.read(self.file, reference.at)?;
        let owner = u64::try_from(reference.column).ok().map(|c| (id, c));
        let found = page.get(..KEY).zip(page.get(KEY..OWNER)).and_then(|(row, column)| {
            let row = RowId::from_le_bytes(<[u8; KEY]>::try_from(row).ok()?);
            let column = u32::from_le_bytes(<[u8; 4]>::try_from(column).ok()?);
            Some((row, u64::from(column)))
        });
        if found.is_none() || found != owner {
            return Err(corrupt(&format!("page {} does not hold column {} of row {id}", reference.at, reference.column)));
        }
        Ok(self.sealing.plain_value(id, page.get(OWNER..).unwrap_or_default())?.into_owned())
    }

    /// Frees `pages`. One that cannot be freed is left: a change already
    /// made never fails for it.
    pub(in crate::engine::rows) fn free(&self, pages: &[PageId]) {
        for &page in pages {
            let _ = self.pool.free(self.file, page);
        }
    }

    /// Frees the pages of the values `record`, row `id`'s, keeps out of line.
    pub(in crate::engine::rows) fn free_values(&self, id: RowId, record: &[u8]) {
        if let Ok(references) = self.sealing.plain_record(id, record).and_then(|plain| self.format.outside(&plain)) {
            let pages: Vec<PageId> = references.iter().map(|r| PageId(r.at)).collect();
            self.free(&pages);
        }
    }

    /// The row `record` holds, row `id`'s, with `columns` decoded, or all.
    pub(in crate::engine::rows) fn decode(&self, id: RowId, record: &[u8], columns: Option<&[usize]>) -> Result<Row> {
        let mut fetch = |reference: Reference| self.get_value(id, reference);
        let record = self.sealing.plain_record(id, record)?;
        let mut row = match columns {
            Some(columns) => self.format.decode_columns_with(&record, columns, &mut fetch)?,
            None => self.format.decode_with(&record, &mut fetch)?,
        };
        if !self.lobs.columns().is_empty() {
            self.lobs.resolve(&mut row);
        }
        Ok(row)
    }

    /// Appends to `out` the values of `columns` of the row `record` holds,
    /// row `id`'s, in their order, and no other (`record/listed.rs`,
    /// RULES.md D862).
    pub(in crate::engine::rows) fn decode_listed(&self, (id, record): (RowId, &[u8]), columns: &[usize], out: &mut Row) -> Result<()> {
        let mut fetch = |reference: Reference| self.get_value(id, reference);
        let record = self.sealing.plain_record(id, record)?;
        let base = out.len();
        self.format.decode_listed_with(&record, columns, &mut fetch, out)?;
        if !self.lobs.columns().is_empty() {
            for (value, &column) in out.iter_mut().skip(base).zip(columns) {
                if let Ok(one) = <&mut [Value; 1]>::try_from(std::slice::from_mut(value)) {
                    self.lobs.resolve_column(column, one);
                }
            }
        }
        Ok(())
    }

    /// Each of `columns`, given with a place of the caller's, of the row
    /// `record` holds, row `id`'s, decoded alone and handed to `each` with
    /// its place: no row is made, as a zone's sum up reads a few columns of
    /// every version of its rows (`rows/zones/loose.rs`). Not for a column
    /// whose long objects `decode` resolves.
    pub(in crate::engine::rows) fn each_column(
        &self,
        (id, record): (RowId, &[u8]),
        columns: &[(usize, usize)],
        each: &mut impl FnMut(usize, &Value),
    ) -> Result<()> {
        let mut fetch = |reference: Reference| self.get_value(id, reference);
        let record = self.sealing.plain_record(id, record)?;
        for &(place, column) in columns {
            each(place, &self.format.column_with(&record, column, &mut fetch)?);
        }
        Ok(())
    }

    /// Adds the row `record` holds, row `id`'s, to `into`, each column
    /// decoded straight into its vector (`gathered.rs`).
    pub(in crate::engine::rows) fn gather_record(&self, (id, record): (RowId, &[u8]), into: &mut Gathering) -> Result<()> {
        self.gather_plain((id, &self.sealing.plain_record(id, record)?), into)
    }

    /// As `gather_record`, of a record already unsealed (`plain_record`).
    pub(in crate::engine::rows) fn gather_plain(&self, (id, plain): (RowId, &[u8]), into: &mut Gathering) -> Result<()> {
        let mut fetch = |reference: Reference| self.get_value(id, reference);
        into.push((&self.format, plain), &mut fetch, Some(&self.lobs))
    }

    /// The record `record` holds, row `id`'s, unsealed.
    pub(in crate::engine::rows) fn plain(&self, id: RowId, record: &[u8]) -> Result<Vec<u8>> {
        Ok(self.sealing.plain_record(id, record)?.into_owned())
    }
}
