//! Values of a row-store table's records (RULES.md D31): a row encoded,
//! its long values kept out of line on pages of their own, each page
//! checked against its row and column as it is read, and freed with its
//! row; and a record decoded, all its columns or those asked for. Split
//! from rows.rs (RULES.md R-SIZE-2).

use super::gathered::Gathering;
use super::{KEY, OWNER, RowTable, corrupt};
use crate::disk::pager::PageId;
use crate::engine::record::{Reference, Spill};
use crate::error::{Code, Result, SqlError};
use crate::storage::{Conjunction, Row, RowId};

impl RowTable {
    /// The record of row `id`, holding `row`, and the pages of its values
    /// out of line; on failure, none is left allocated.
    pub(in crate::engine::rows) fn encode(&self, id: RowId, row: &Row) -> Result<(Vec<u8>, Vec<PageId>)> {
        let mut pages = Vec::new();
        let mut record = Vec::new();
        // A record sealed takes more: room is kept for it.
        let spill = Spill { most: self.spill.most.saturating_sub(self.sealing.overhead()), ..self.spill };
        let encoded = self.format.encode_spilling(row, spill, &mut record, |column, bytes| {
            let page = self.put_value(id, column, bytes)?;
            pages.push(page);
            Ok(page.0)
        });
        match encoded.and_then(|()| self.sealing.record(id, record)) {
            Ok(stored) => Ok((stored, pages)),
            Err(e) => {
                self.free(&pages);
                Err(e)
            }
        }
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
        match columns {
            Some(columns) => self.format.decode_columns_with(&record, columns, &mut fetch),
            None => self.format.decode_with(&record, &mut fetch),
        }
    }

    /// Adds the row `record` holds, row `id`'s, to `into`, each column
    /// decoded straight into its vector (`gathered.rs`).
    pub(in crate::engine::rows) fn gather_record(&self, (id, record): (RowId, &[u8]), into: &mut Gathering) -> Result<()> {
        self.gather_plain((id, &self.sealing.plain_record(id, record)?), into)
    }

    /// As `gather_record`, of a record unsealed (`plain_matching`).
    pub(in crate::engine::rows) fn gather_plain(&self, (id, plain): (RowId, &[u8]), into: &mut Gathering) -> Result<()> {
        let mut fetch = |reference: Reference| self.get_value(id, reference);
        into.push((&self.format, plain), &mut fetch)
    }

    /// The record `record` holds, row `id`'s, unsealed, if `condition`, if
    /// given, matches its row, only the condition's columns decoded.
    pub(in crate::engine::rows) fn plain_matching(
        &self,
        (id, record): (RowId, &[u8]),
        condition: Option<&Conjunction>,
    ) -> Result<Option<Vec<u8>>> {
        let plain = self.sealing.plain_record(id, record)?;
        if let Some(condition) = condition {
            let mut fetch = |reference: Reference| self.get_value(id, reference);
            let row = self.format.decode_columns_with(&plain, &condition.columns(), &mut fetch)?;
            if !condition.matches(&row) {
                return Ok(None);
            }
        }
        Ok(Some(plain.into_owned()))
    }
}
