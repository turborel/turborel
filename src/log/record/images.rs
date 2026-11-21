//! \[DESIGN\] An update's two images in its log record (RULES.md D475):
//! the row after whole, and of the row before only the columns the update
//! changed, each its position and its value as it was, so an update of a
//! few columns of a wide row logs the row once, not twice. Read back, the
//! row before is the row after with those columns put back, every value
//! as it was written. A row before of another width than the row after,
//! as a table altered since leaves it, is written whole:
//!
//! ```text
//! update body:  table | generation u32 | row id u64 | after (row)
//!               | WHOLE u8 | before (row)
//!               | CHANGED u8 | count u32 | (position u32 | value)...
//! ```
//!
//! A column is changed if its value's bytes differ, so values that compare
//! equal but are written apart, as a decimal of another scale, are kept.

use super::{Change, put_table, table};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::storage::{Row, RowId};
use crate::value::codec::{put_row, put_value, row, value};

/// The row before follows whole.
const WHOLE: u8 = 0;
/// Only the columns of the row before that changed follow.
const CHANGED: u8 = 1;

/// Writes an update's body: its row, the row after whole, and of the row
/// before the columns changed, or all of it if its width differs.
pub(super) fn put_update(out: &mut Vec<u8>, (name, generation, id): (&QualifiedName, u32, RowId), before: &Row, after: &Row) {
    put_table(out, name);
    out.put_u32(generation);
    out.put_u64(id);
    out.put_u32(u32::try_from(after.len()).unwrap_or(u32::MAX));
    let mut spans = Vec::with_capacity(after.len());
    for v in after {
        let start = out.len();
        put_value(out, v);
        spans.push(start..out.len());
    }
    if before.len() != after.len() {
        out.put_u8(WHOLE);
        put_row(out, before);
        return;
    }
    let mut changed = Vec::new();
    let mut count = 0_u32;
    let mut was = Vec::new();
    for (at, (v, span)) in before.iter().zip(spans).enumerate() {
        was.clear();
        put_value(&mut was, v);
        if out.get(span).is_none_or(|now| *now != *was) {
            changed.put_u32(u32::try_from(at).unwrap_or(u32::MAX));
            changed.extend_from_slice(&was);
            count = count.saturating_add(1);
        }
    }
    out.put_u8(CHANGED);
    out.put_u32(count);
    out.extend_from_slice(&changed);
}

/// Reads an update's body written by `put_update`.
pub(super) fn update(r: &mut Reader<'_>) -> Result<Change> {
    let (name, generation, id) = (table(r)?, r.u32()?, r.u64()?);
    let after = row(r)?;
    let before = match r.u8()? {
        WHOLE => row(r)?,
        CHANGED => changed(r, &after)?,
        other => return Err(malformed(&format!("of an unknown form {other}"))),
    };
    Ok(Change::Update { table: name, generation, row: id, before, after })
}

/// The row before: `after` with the columns read put back.
fn changed(r: &mut Reader<'_>, after: &Row) -> Result<Row> {
    let mut before = after.clone();
    for _ in 0..r.u32()? {
        let at = usize::try_from(r.u32()?).unwrap_or(usize::MAX);
        let was = value(r)?;
        *before.get_mut(at).ok_or_else(|| malformed("changing a column past the row's"))? = was;
    }
    Ok(before)
}

/// SQLCODE -916: an update's record is malformed, `why`.
fn malformed(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a log record of an update is malformed: its row before {why}"))
}

// Unit tests: an update's images read back as written, of the columns
// changed alone, and malformed ones refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::super::{Change, Record};
    use crate::catalog::QualifiedName;
    use crate::error::Code;
    use crate::storage::Row;
    use crate::value::types::TypeRegistry;
    use crate::value::{Decimal, Value};

    /// The record of an update of row 7 of table `T`.
    fn update(before: Row, after: Row) -> Record {
        let table = QualifiedName { auth: "A".into(), ident: "T".into() };
        Record { tx: 3, change: Change::Update { table, generation: 1, row: 7, before, after } }
    }

    /// A row of ten columns, the first `n`, the others wide.
    fn wide(n: i128) -> Row {
        let mut row = vec![Value::Exact(Decimal::new(n, 0))];
        row.extend((0_u8..9).map(|c| Value::Char(format!("{c:-<40}"))));
        row
    }

    /// Verifies: [DESIGN] RULES.md D475, an update logs of its row before
    /// only the columns changed: read back, both images are as written,
    /// value for value, the bytes of each; values that compare equal but
    /// are written apart (a decimal of another scale, zero and negative
    /// zero) count as changed, a NaN written alike as not; a row before
    /// of another width is written whole; an update of one column of ten
    /// takes little more than an insert of the row. MC/DC of a column
    /// kept: bytes differ (kept) against alike (not).
    #[test]
    fn an_update_logs_the_columns_it_changed() {
        let types = TypeRegistry::default();
        let back = |record: &Record| format!("{:?}", Record::decode(&record.encode(), &types).unwrap().change);
        let before =
            vec![Value::Exact(Decimal::new(10, 1)), Value::Approx(0.0), Value::Approx(f64::NAN), Value::Null, Value::Char("x".into())];
        let after =
            vec![Value::Exact(Decimal::new(1, 0)), Value::Approx(-0.0), Value::Approx(f64::NAN), Value::Null, Value::Char("y".into())];
        for record in [update(before.clone(), after.clone()), update(vec![], after.clone()), update(after.clone(), after)] {
            assert_eq!(back(&record), format!("{:?}", record.change));
        }
        let one = update(wide(1), wide(2)).encode().len();
        let table = QualifiedName { auth: "A".into(), ident: "T".into() };
        let insert = Record { tx: 3, change: Change::Insert { table, generation: 1, row: 7, after: wide(2) } }.encode().len();
        assert!(one < insert + 40, "{one} bytes against an insert's {insert}");
    }

    /// Verifies: [DESIGN] RULES.md D475, an update's record whose changed
    /// column lies past its row, or of an unknown form, is refused
    /// (SQLCODE -916).
    #[test]
    fn a_malformed_update_is_refused() {
        let types = TypeRegistry::default();
        let mut bytes = update(vec![Value::Null], vec![Value::Char("x".into())]).encode();
        // A byte of the changed column's position, before its null value
        // of one byte: the column then past the row of one.
        let position = bytes.len() - 2;
        bytes[position] = 9;
        assert_eq!(Record::decode(&bytes, &types).err().map(|e| e.code), Some(Code::Corrupt));
        let mut unknown = update(vec![Value::Null], vec![Value::Null]).encode();
        let form = unknown.len() - 5;
        unknown[form] = 7;
        assert_eq!(Record::decode(&unknown, &types).err().map(|e| e.code), Some(Code::Corrupt));
    }
}
