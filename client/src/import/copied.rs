//! \[DESIGN\] An import by bulk copy (RULES.md D59, D195): its rows checked
//! as the INSERT's parameters take them, then sent a batch to a copy,
//! each copy committing its batch as it ends, with no round trip but the
//! copy's end. A row the server refuses fails its copy, every row of the
//! batch undone, the failure's detail naming the row, so the import says
//! its record as it would of a row inserted alone.

use std::io::{Read, Write};

use turborel::params::ParameterType;
use turborel::storage::Row;
use turborel::value::DataType;
use turborel_protocol::Failure;
use turborel_protocol::capability::COPY;

use super::insert::{Cause, Import, ImportError, Imported, assigned};
use super::{Position, Source};
use crate::connection::Connection;
use crate::copy::Copying;
use crate::statements::Statement;

/// What the server's detail of a row a copy refused begins with, and ends.
const ROW: &str = "row ";
const OF_THE_COPY: &str = " of the copy";

/// Whether an import into `insert`'s parameters, as `how` says, goes by
/// copy: granted, no bad row skipped, no large object column.
pub(super) fn copies<S: Read + Write>(connection: &Connection<S>, insert: &Statement, how: &Import) -> bool {
    let lob = |p: &ParameterType| matches!(p.clone().data_type(), DataType::Lob { .. });
    connection.granted().has(COPY) && !how.skip && !insert.parameters().iter().any(lob)
}

/// The row of a copy `failure` names, from 1, if it names one.
fn row_of(failure: &Failure) -> Option<usize> {
    let detail = failure.diagnostics.as_ref()?.detail.as_deref()?;
    detail.strip_prefix(ROW)?.strip_suffix(OF_THE_COPY)?.parse().ok()
}

/// An import by copy under way.
struct Run<'a> {
    how: &'a Import,
    types: Vec<ParameterType>,
    committed: u64,
    /// Where each record of the batch under way began.
    batch: Vec<Position>,
}

impl Run<'_> {
    /// Stops the import at `position` for `cause`, `rolled_back` rows of
    /// the batch undone.
    fn stop(&self, position: Option<Position>, cause: Cause, rolled_back: usize) -> ImportError {
        let rolled_back = u64::try_from(rolled_back).unwrap_or(u64::MAX);
        ImportError { position, cause, committed: self.committed, rolled_back }
    }

    /// Ends `copying`, the batch's copy: its rows committed, or why it
    /// failed, at the row it names.
    fn ended<S: Read + Write>(&mut self, copying: Copying<'_, S>) -> Result<(), ImportError> {
        match copying.finish() {
            Ok(rows) => {
                self.committed = self.committed.saturating_add(rows);
                self.batch.clear();
                Ok(())
            }
            Err(failure) => {
                let row = row_of(&failure).filter(|&n| n > 0 && n <= self.batch.len());
                let (position, before) =
                    row.map_or((None, self.batch.len()), |n| (self.batch.get(n.saturating_sub(1)).copied(), n.saturating_sub(1)));
                Err(self.stop(position, Cause::sql(failure), before))
            }
        }
    }
}

/// Imports `source`'s records into `table`'s `columns`, as SQL names them,
/// of the parameters' types `types`, as `how` says, over `connection`,
/// which has no transaction active.
///
/// # Errors
///
/// As `insert::import`'s.
pub(super) fn import<S: Read + Write, R: Source>(
    connection: &mut Connection<S>,
    source: &mut R,
    (how, table, columns): (&Import, &str, Vec<String>),
    types: Vec<ParameterType>,
) -> Result<Imported, ImportError> {
    let mut run = Run { how, types, committed: 0, batch: Vec::new() };
    while run.copied(connection, source, (table, &columns))? {}
    Ok(Imported { rows: run.committed, rejected: 0 })
}

impl Run<'_> {
    /// The next of `source`'s records, checked as the INSERT's parameters
    /// take it: where it began, and its values.
    fn checked<R: Source>(&self, source: &mut R) -> Result<Option<(Position, Row)>, ImportError> {
        let record = source.record().map_err(|e| self.stop(Some(e.position), Cause::Input(e.message), self.batch.len()))?;
        let Some(record) = record else { return Ok(None) };
        let wanted = self.types.len();
        if record.values.len() != wanted {
            let msg = format!("[DESIGN] {} fields, for {wanted} columns", record.values.len());
            return Err(self.stop(Some(record.position), Cause::Input(msg), self.batch.len()));
        }
        let values =
            assigned(&record.values, &self.types).map_err(|f| self.stop(Some(record.position), Cause::sql(f), self.batch.len()))?;
        Ok(Some((record.position, values)))
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
        let Some(first) = self.checked(source)? else { return Ok(false) };
        let mut copy = connection.copy_begin(table, columns.to_vec(), true).map_err(|f| self.stop(None, Cause::sql(f), 0))?;
        let mut next = Some(first);
        while let Some((at, values)) = next {
            copy.send(values).map_err(|f| self.stop(Some(at), Cause::sql(f), self.batch.len()))?;
            self.batch.push(at);
            if u64::try_from(self.batch.len()).unwrap_or(u64::MAX) == self.how.batch {
                return self.ended(copy).map(|()| true);
            }
            next = self.checked(source)?;
        }
        self.ended(copy).map(|()| false)
    }
}
