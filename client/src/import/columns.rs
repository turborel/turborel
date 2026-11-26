//! \[DESIGN\] An import by bulk copy by column (RULES.md D1148): where the
//! server grants `columns` and the source splits its records itself, each
//! record's fields go straight into `Cells` as they are split, and the
//! cells go in a CopyColumns once they are about `CHUNK_BYTES`; no value,
//! and no string, is made of a field, and the cells' buffers are kept
//! from batch to batch. The server parses each column straight into its
//! type.
//!
//! Each field's text is what the rows of a copy would send as a
//! character string, and a null what they would send as null, so the
//! rows inserted, and the failures, are those of a copy by row
//! (`copied.rs`): a batch to a copy, a record of too many or too few
//! fields refused before its batch is sent, a row the server refuses
//! named by its record. The client casts nothing: a string too long for
//! its column is refused by the server, which assigns it as INSERT does.
//! A binary string column is left to the copy by row, which reads its
//! hexits into octets (RULES.md D976) before it sends them.

use std::io::{Read, Write};

use turborel::params::ParameterType;
use turborel_protocol::capability::COLUMNS;

use super::copied::{Kept, miscounted};
use super::insert::{Cause, Import, ImportError, Imported};
use super::{Cells, Position, Source};
use crate::connection::Connection;
use crate::copy::CHUNK_BYTES;

/// Whether an import by copy into parameters of `types`, from `source`,
/// goes by column: `columns` granted, the source splitting its records
/// itself, and no column a binary string's.
pub(super) fn by_column<S: Read + Write, R: Source>(connection: &Connection<S>, source: &R, types: &[ParameterType]) -> bool {
    connection.granted().has(COLUMNS) && source.splits() && !types.iter().any(|t| t.clone().data_type().is_binary())
}

/// An import by column under way.
struct Run<'a> {
    how: &'a Import,
    kept: Kept,
    cells: Cells,
}

/// Imports `source`'s records into `table`'s `columns`, as SQL names them,
/// as `how` says, over `connection`, which has no transaction active.
///
/// # Errors
///
/// As `insert::import`'s.
pub(super) fn import<S: Read + Write, R: Source>(
    connection: &mut Connection<S>,
    source: &mut R,
    (how, table, columns): (&Import, &str, Vec<String>),
) -> Result<Imported, ImportError> {
    let mut run = Run { how, kept: Kept::default(), cells: Cells::new(columns.len()) };
    while run.copied(connection, source, (table, &columns))? {}
    Ok(Imported { rows: run.kept.committed, rejected: 0 })
}

impl Run<'_> {
    /// Appends the next of `source`'s records to the cells, checked to have
    /// a field for each column: where it began, or none at the end.
    fn next<R: Source>(&mut self, source: &mut R) -> Result<Option<Position>, ImportError> {
        self.cells.begin();
        let read = source.record_into(&mut self.cells).map_err(|e| self.kept.stopped(Some(e.position), Cause::Input(e.message)))?;
        let Some(position) = read else { return Ok(None) };
        let (fields, wanted) = (self.cells.fields(), self.cells.columns());
        if fields != wanted {
            return Err(self.kept.stopped(Some(position), Cause::Input(miscounted(fields, wanted))));
        }
        self.cells.row();
        Ok(Some(position))
    }

    /// Copies a batch of `source`'s records into `table`'s `columns`, its
    /// copy begun at its first record and ended at its last, abandoned if
    /// a record fails first: whether records may follow.
    fn copied<S: Read + Write, R: Source>(
        &mut self,
        connection: &mut Connection<S>,
        source: &mut R,
        (table, columns): (&str, &[String]),
    ) -> Result<bool, ImportError> {
        // A record that fails leaves its fields in the cells: the import
        // stops, and the cells go with it.
        let Some(first) = self.next(source)? else { return Ok(false) };
        let mut copy = connection.copy_begin(table, columns.to_vec(), true).map_err(|f| self.kept.stop(None, Cause::sql(f), 0))?;
        let mut next = Some(first);
        while let Some(at) = next {
            self.kept.batch.push(at);
            let whole = self.kept.whole(self.how);
            if whole || self.cells.bytes() >= CHUNK_BYTES {
                self.cells.send(&mut copy).map_err(|f| self.kept.stopped(Some(at), Cause::sql(f)))?;
            }
            if whole {
                return self.kept.ended(copy).map(|()| true);
            }
            next = self.next(source)?;
        }
        self.cells.send(&mut copy).map_err(|f| self.kept.stopped(None, Cause::sql(f)))?;
        self.kept.ended(copy).map(|()| false)
    }
}
