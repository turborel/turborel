//! \[EXTENSION\] RULES.md D106: the rows of an XML index's query that it
//! answers of each row from its entries alone (`Index::xml_answers`): a
//! row it holds of is kept as its visible version is fetched; one it may
//! hold of is checked by the query's own test of that version's value
//! (`XmlQuery::matches`); one it does not give is none the query holds of.

use std::sync::Arc;

use super::entries::{Entries, Reading, fetch};
use super::{RowIter, broken, failed};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::error::Result;
use crate::index::xml::{XmlAnswer, XmlProbe};
use crate::index::{Key, Probe};
use crate::storage::{Condition, Row, RowId};
use crate::txn::{TxId, checked};
use crate::value::Value;

/// The rows of the query `asked` that the index `index` of `table`
/// answers, as `tx` sees them, each of the columns `needed`, followed by
/// its id if `located`.
#[inline(never)]
pub(super) fn answered(
    db: &Arc<State>,
    tx: TxId,
    (table, index, asked): (&QualifiedName, &str, &XmlProbe),
    (needed, located): (&[usize], bool),
) -> RowIter {
    let rows = answered_entries(db, tx, (table, index, asked), Some(needed));
    rows.map_or_else(failed, |rows| Box::new(rows.map(move |entry| entry.map(|read| super::scan::read_row(located, read)))))
}

/// The rows, with their ids, of the query `asked` that the index `index`
/// of `table` answers, as `tx` sees them, of the columns `needed` and the
/// index's, or every column.
///
/// # Errors
///
/// SQLCODE -910 if the table or index does not exist, or the index
/// answers no query.
pub(super) fn answered_entries(
    db: &Arc<State>,
    tx: TxId,
    (table, index, asked): (&QualifiedName, &str, &XmlProbe),
    needed: Option<&[usize]>,
) -> Result<Entries> {
    let storage = Arc::clone(db.storage_of(table)?);
    let found = db.index_of(index)?;
    let shape = db.catalog.index(index).map(IndexDesc::key_shape);
    let column = shape.as_ref().and_then(|s| s.columns.first().copied());
    let (Some(ix), Some(column), Some(answers)) = (shape, column, Arc::clone(found).xml_answers(&asked.query)) else {
        return Err(broken("an XML index of one column answers its query"));
    };
    // D34, D106: storage cannot tell which rows the query selects.
    storage.read_by_condition(tx, &Condition { columns: Vec::new(), probe: Probe::All })?;
    let mut reading = Reading::of(&ix, needed, found.key_projection());
    let query = asked.query.clone();
    let rows = checked(answers, db.tx.stop_of(tx)).filter_map(move |answer| {
        let kept = |(id, given)| -> Result<Option<(RowId, Row)>> {
            let Some((_, row)) = fetch(&*storage, &ix, &mut reading, (tx, id), &Key(vec![Value::Null]))? else { return Ok(None) };
            // The version seen holds a value, null of none, which no query
            // holds of [ISO/IEC 9075-14:2006 draft §8.4 GR2].
            let Some(Value::Xml(value)) = row.get(column) else { return Ok(None) };
            if given == XmlAnswer::Check && !query.matches(value)? {
                return Ok(None);
            }
            Ok(Some((id, row)))
        };
        answer.and_then(kept).transpose()
    });
    Ok(Box::new(checked(rows, db.tx.stop_of(tx))))
}
