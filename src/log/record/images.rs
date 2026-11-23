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

use std::cell::RefCell;
use std::ops::Range;

use super::{Change, put_table, table};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::storage::{AfterImage, Row, RowId};
use crate::value::codec::{put_row, put_value, row, value};

/// The row before follows whole.
const WHOLE: u8 = 0;
/// Only the columns of the row before that changed follow.
const CHANGED: u8 = 1;

/// Writes an update's body: its row, the row after whole, and of the row
/// before the columns changed, or all of it if its width differs.
pub(super) fn put_update(out: &mut Vec<u8>, head: (&QualifiedName, u32, RowId), before: &Row, after: &Row) {
    put_images(out, head, (before, after), None);
}

/// \[DESIGN\] (RULES.md D875) As `put_update`, of a change that may differ
/// only in the columns `set`, in ascending order, each once: of the row
/// before, only those are read, so one of which only they hold their
/// values gives the bytes `put_update` gives of the rows whole. A row
/// before of another width than the row after is written whole, as it
/// is given.
pub fn put_update_of(out: &mut Vec<u8>, head: (&QualifiedName, u32, RowId), before: &Row, after: &Row, set: &[usize]) {
    put_images(out, head, (before, after), Some(set));
}

/// \[DESIGN\] (RULES.md D916) As `put_update_of`, the row after given as
/// `image`, its values as they are written here and where those of the
/// columns set lie: of an update that holds the values it set alone, whose
/// table gave the image. The bytes are those `put_update_of` writes of the
/// row the image is of.
pub fn put_update_image(out: &mut Vec<u8>, (name, generation, id): (&QualifiedName, u32, RowId), before: &Row, image: &AfterImage) {
    let (values, columns, set) = image.parts();
    put_table(out, name);
    out.put_u32(generation);
    out.put_u64(id);
    out.put_u32(u32::try_from(columns).unwrap_or(u32::MAX));
    let base = out.len();
    out.extend_from_slice(values);
    if before.len() != columns {
        out.put_u8(WHOLE);
        put_row(out, before);
        return;
    }
    let mut spans = SPANS.take();
    spans.clear();
    spans.extend(set.iter().map(|(at, span)| (*at, base.saturating_add(span.start)..base.saturating_add(span.end))));
    put_changed(out, before, &spans);
    SPANS.set(spans);
}

/// `put_update`'s body, of the columns `set` of the row before alone if
/// given, else of them all.
fn put_images(
    out: &mut Vec<u8>,
    (name, generation, id): (&QualifiedName, u32, RowId),
    (before, after): (&Row, &Row),
    set: Option<&[usize]>,
) {
    put_table(out, name);
    out.put_u32(generation);
    out.put_u64(id);
    out.put_u32(u32::try_from(after.len()).unwrap_or(u32::MAX));
    if before.len() != after.len() {
        put_row_values(out, after);
        out.put_u8(WHOLE);
        put_row(out, before);
        return;
    }
    // Where each value of the row after that may have changed lies, with
    // its column, in a vector its thread keeps, so an update's record
    // allocates none for them.
    let mut spans = SPANS.take();
    spans.clear();
    let mut wanted = set.map(|columns| columns.iter().copied().peekable());
    for (at, v) in after.iter().enumerate() {
        let start = out.len();
        put_value(out, v);
        if wanted.as_mut().is_none_or(|left| left.next_if_eq(&at).is_some()) {
            spans.push((at, start..out.len()));
        }
    }
    put_changed(out, before, &spans);
    SPANS.set(spans);
}

thread_local! {
    /// The places of the values of the last row after an update's record
    /// wrote on this thread, each with its column: kept for the next,
    /// which clears it.
    static SPANS: RefCell<Vec<(usize, Range<usize>)>> = const { RefCell::new(Vec::new()) };
}

/// Writes each value of `row`, as `put_row` does after its count.
fn put_row_values(out: &mut Vec<u8>, row: &Row) {
    for v in row {
        put_value(out, v);
    }
}

/// Writes of `before` the columns of `spans` whose bytes are not those at
/// their place there, of the row after as `out` holds it.
fn put_changed(out: &mut Vec<u8>, before: &Row, spans: &[(usize, Range<usize>)]) {
    // [DESIGN] RULES.md D822: each column of the row before written in
    // place after the count, and taken back if its bytes are those it
    // has after.
    out.put_u8(CHANGED);
    let count_at = out.len();
    out.put_u32(0);
    let mut count = 0_u32;
    for (at, span) in spans {
        let Some(v) = before.get(*at) else { continue };
        let mark = out.len();
        out.put_u32(u32::try_from(*at).unwrap_or(u32::MAX));
        let value_at = out.len();
        put_value(out, v);
        let (written, was) = out.split_at(value_at);
        if written.get(span.clone()).is_some_and(|now| now == was) {
            out.truncate(mark);
        } else {
            count = count.saturating_add(1);
        }
    }
    if let Some(place) = out.get_mut(count_at..count_at.saturating_add(4)) {
        place.copy_from_slice(&count.to_le_bytes());
    }
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
        row.extend((0_u8..9).map(|c| Value::Char(format!("{c:-<40}").into())));
        row
    }

    /// Verifies: [DESIGN] RULES.md D475, D822, an update logs of its row before
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

    /// Verifies: [DESIGN] RULES.md D875, `put_update_of`: given the
    /// columns set, in order, and a row before holding only their values,
    /// nulls elsewhere, it writes the bytes `put_update` writes of the
    /// rows whole: of one column set and changed, of two set with one
    /// left as it was, of none set, and of a row before of another width.
    #[test]
    fn an_update_of_the_columns_set_writes_the_same_bytes() {
        let table = QualifiedName { auth: "A".into(), ident: "T".into() };
        let head = (&table, 1_u32, 7_u64);
        let (before, mut after) = (wide(1), wide(1));
        after[0] = Value::Exact(Decimal::new(2, 0));
        after[4] = Value::Char("changed".into());
        // Whether the record of the rows whole is that of the columns `set` of `sparse`.
        let same = |was: &Row, sparse: &Row, now: &Row, set: &[usize]| {
            let (mut whole, mut of_set) = (vec![0xEE], vec![0xEE]);
            super::put_update(&mut whole, head, was, now);
            super::put_update_of(&mut of_set, head, sparse, now, set);
            whole == of_set
        };
        for set in [vec![0_usize, 4], vec![0, 3, 4], vec![0, 4, 9]] {
            let mut sparse = vec![Value::Null; before.len()];
            for &c in &set {
                sparse[c] = before[c].clone();
            }
            assert!(same(&before, &sparse, &after, &set), "{set:?}");
        }
        assert!(same(&before, &vec![Value::Null; before.len()], &before, &[]), "none set");
        assert!(same(&vec![Value::Null], &vec![Value::Null], &after, &[0]), "another width");
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
