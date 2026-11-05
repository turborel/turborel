//! \[DESIGN\] RULES.md D31, D38: the records a grouping by batch writes
//! past its work area, and reads back. A record of rows: a tag, their
//! count and numbers, then each column, of a vector of integers its
//! units, scale and nulls, of any other its values (`codec::put_value`).
//! A record of pieces, groups partly made: a tag, their count, then each
//! piece: its first row's number, its grouping values, then each set
//! function's state (`Accumulator::write_state`).

use super::super::table::Held;
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::storage::{Batch, Column, Ints, Row};
use crate::value::Value;
use crate::value::codec::{put_row, put_value, row, value};
use crate::value::types::TypeRegistry;

/// The tag of a record of rows, and of one of pieces.
const ROWS_TAG: u8 = 0;
const PIECE_TAG: u8 = 1;

/// The tag of a column written as values, and of one written as integers.
const VALUES: u8 = 0;
const INTEGERS: u8 = 1;

/// What a record holds: rows of a batch, and their numbers; or pieces.
pub(super) enum Record {
    Rows(Batch, Vec<u64>),
    Pieces(Vec<Piece>),
}

/// A group partly made, as written: its first row's number, its grouping
/// values, and each set function's state, in order.
pub(in crate::exec::batch::groups) struct Piece {
    pub(in crate::exec::batch::groups) first: u64,
    pub(in crate::exec::batch::groups) values: Row,
    pub(in crate::exec::batch::groups) states: Vec<Vec<u8>>,
}

/// What the record `record` holds, its values of types beyond the standard
/// found among `types`.
///
/// # Errors
///
/// SQLCODE -916 if it is not a record `put_batch` or `put_pieces` wrote.
pub(super) fn record_of(record: &[u8], types: &TypeRegistry) -> Result<Record> {
    let (&tag, rest) = record.split_first().ok_or_else(malformed)?;
    match tag {
        ROWS_TAG => batch_of(rest, types).map(|(batch, numbers)| Record::Rows(batch, numbers)),
        PIECE_TAG => pieces_of(rest, types).map(Record::Pieces),
        _ => Err(malformed()),
    }
}

/// Appends to `out` `group`, partly made, as a piece: its first row's
/// number, its grouping values, then each set function's state, each after
/// its length; whether each has one.
pub(super) fn put_group((first, values, states): &Held, out: &mut Vec<u8>) -> bool {
    out.put_u64(*first);
    put_row(out, values);
    out.put_u32(u32::try_from(states.len()).unwrap_or(u32::MAX));
    for state in states {
        let at = out.len();
        out.put_u32(0);
        if !state.write_state(out) {
            return false;
        }
        let len = u32::try_from(out.len().saturating_sub(at).saturating_sub(LENGTH_BYTES)).unwrap_or(u32::MAX);
        if let Some(field) = out.get_mut(at..at.saturating_add(LENGTH_BYTES)) {
            field.copy_from_slice(&len.to_le_bytes());
        }
    }
    true
}

/// The bytes of a state's length before it in a piece, a `u32`.
const LENGTH_BYTES: usize = 4;

/// Appends `piece`, read back, to `out`, as `put_group` wrote it.
pub(super) fn put_piece(piece: &Piece, out: &mut Vec<u8>) {
    out.put_u64(piece.first);
    put_row(out, &piece.values);
    out.put_u32(u32::try_from(piece.states.len()).unwrap_or(u32::MAX));
    for written in &piece.states {
        out.put_u32(u32::try_from(written.len()).unwrap_or(u32::MAX));
        out.extend_from_slice(written);
    }
}

/// Writes to `out`, emptied first, a record of `count` pieces, `pieces`
/// as `put_group` and `put_piece` appended them.
pub(super) fn put_pieces(count: u32, pieces: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.push(PIECE_TAG);
    out.put_u32(count);
    out.extend_from_slice(pieces);
}

/// The pieces `record`, its tag read, holds, as `put_pieces` wrote them.
fn pieces_of(record: &[u8], types: &TypeRegistry) -> Result<Vec<Piece>> {
    let mut reader = Reader::new(record, "spilled groups").with_types(types);
    let count = reader.u32()?;
    let mut pieces = Vec::new();
    for _ in 0..count {
        pieces.push(piece_of(&mut reader)?);
    }
    reader.finish()?;
    Ok(pieces)
}

/// The next piece `reader` reads, as `put_piece` wrote it.
fn piece_of(reader: &mut Reader<'_>) -> Result<Piece> {
    let first = reader.u64()?;
    let values = row(reader)?;
    let count = reader.u32()?;
    let mut states = Vec::new();
    for _ in 0..count {
        let len = usize::try_from(reader.u32()?).map_err(|_| malformed())?;
        states.push(reader.take(len)?.to_vec());
    }
    Ok(Piece { first, values, states })
}

/// The bytes of a row's number, a `u64`, the least a record holds of each
/// of its rows: a count of rows past its bytes is not believed.
const NUMBER_BYTES: usize = 8;

/// Writes to `out`, emptied first, the rows `rows` of `batch`, each its
/// partition, place and number, as a record: their count and numbers,
/// then each column's tag and values.
pub(super) fn put_batch(batch: &Batch, rows: &[(usize, usize, u64)], out: &mut Vec<u8>) {
    out.clear();
    out.push(ROWS_TAG);
    out.put_u32(u32::try_from(rows.len()).unwrap_or(u32::MAX));
    for &(.., number) in rows {
        out.put_u64(number);
    }
    out.put_u32(u32::try_from(batch.columns.len()).unwrap_or(u32::MAX));
    for (at, column) in batch.columns.iter().enumerate() {
        if let Column::Ints(ints) = column {
            out.push(INTEGERS);
            out.put_u32(ints.scale);
            let nulled = rows.iter().any(|&(_, r, _)| ints.is_null(r));
            out.push(u8::from(nulled));
            if nulled {
                out.extend(rows.iter().map(|&(_, r, _)| u8::from(ints.is_null(r))));
            }
            for &(_, r, _) in rows {
                out.put_u64(ints.units.get(r).copied().unwrap_or(0).cast_unsigned());
            }
            continue;
        }
        out.push(VALUES);
        for &(_, r, _) in rows {
            match column {
                Column::Values(values) => put_value(out, values.get(r).unwrap_or(&Value::Null)),
                Column::Ints(_) | Column::Floats(_) | Column::Texts(_) | Column::Nulls(_) => {
                    put_value(out, &batch.cell(at, r).to_value());
                }
            }
        }
    }
}

/// The batch a record `put_batch` wrote holds, its tag read, and its
/// rows' numbers, its values of types beyond the standard found among
/// `types`.
///
/// # Errors
///
/// SQLCODE -916 if it is not such a record.
fn batch_of(record: &[u8], types: &TypeRegistry) -> Result<(Batch, Vec<u64>)> {
    let mut reader = Reader::new(record, "spilled batch").with_types(types);
    let rows = usize::try_from(reader.u32()?).map_err(|_| malformed())?;
    if rows > record.len().checked_div(NUMBER_BYTES).unwrap_or(0) {
        return Err(malformed());
    }
    let mut numbers = Vec::with_capacity(rows);
    for _ in 0..rows {
        numbers.push(reader.u64()?);
    }
    let count = reader.u32()?;
    let mut columns = Vec::new();
    for _ in 0..count {
        columns.push(column_of(&mut reader, rows)?);
    }
    reader.finish()?;
    Ok((Batch { columns, rows }, numbers))
}

/// The next column of `rows` rows `reader` reads, as `put_batch` wrote it.
fn column_of(reader: &mut Reader<'_>, rows: usize) -> Result<Column> {
    match reader.u8()? {
        VALUES => {
            let mut values = Vec::with_capacity(rows);
            for _ in 0..rows {
                values.push(value(reader)?);
            }
            Ok(Column::Values(values))
        }
        INTEGERS => {
            let scale = reader.u32()?;
            let nulls = match reader.u8()? {
                0 => None,
                _ => Some(reader.take(rows)?.iter().map(|&b| b != 0).collect()),
            };
            let mut units = Vec::with_capacity(rows);
            for _ in 0..rows {
                units.push(reader.u64()?.cast_signed());
            }
            Ok(Column::Ints(Ints::new(units, scale, nulls)))
        }
        _ => Err(malformed()),
    }
}

/// The error of a record not as `put_batch` wrote it.
pub(super) fn malformed() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a spilled batch's record is malformed")
}

// Unit tests: records of batches and pieces written and read back, and
// records not so (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::super::{Overflow, Queued, RECORD_ROWS};
    use super::*;
    use crate::catalog::MemoryCatalog;
    use crate::db::State;
    use crate::exec::work::Work;
    use crate::functions::{Accumulator, Registry};
    use crate::storage::{Floats, Texts};
    use crate::value::{Bits, DataType, Decimal};

    /// The rows and numbers of `record`, which must be of rows.
    fn rows_of(record: &[u8]) -> Result<(Batch, Vec<u64>)> {
        match record_of(record, &TypeRegistry::default())? {
            Record::Rows(batch, numbers) => Ok((batch, numbers)),
            Record::Pieces(_) => Err(malformed()),
        }
    }

    /// A batch of four rows of a column of each kind: integers, a null
    /// among them; values; approximate numbers; texts, a null among them.
    fn batch() -> Batch {
        let ints = Column::Ints(Ints::new(vec![7_i64, -3_i64, 0_i64, i64::MIN], 2_u32, Some(vec![false, true, false, false])));
        let values = Column::Values(vec![
            Value::Char("a".to_owned()),
            Value::Null,
            Value::Exact(Decimal::new(15_i128, 1_u32)),
            Value::Bits(Bits::of_bits("101").unwrap()),
        ]);
        let floats = Column::Floats(Floats::new(vec![1.5_f64, -0.0_f64, 2.0_f64, 3.0_f64], None));
        let ends = vec![1_usize, 3_usize, 3_usize, 5_usize];
        let texts = Column::Texts(Texts::new("xyzzy".to_owned(), ends, Some(vec![false, false, true, false])));
        Batch { columns: vec![ints, values, floats, texts], rows: 4 }
    }

    /// Verifies: [DESIGN] RULES.md D31, D38: rows of a batch written as a
    /// record (`put_batch`) read back (`batch_of`) as the same rows, in the
    /// order given, each with its number: integers as integers, of their
    /// scale, nulls kept, none marked if none is null; other columns as
    /// their values.
    #[test]
    fn rows_of_a_batch_are_read_back_as_written() {
        let batch = batch();
        let rows = [(0_usize, 3_usize, 30_u64), (0_usize, 1_usize, 10_u64), (0_usize, 0_usize, 0_u64)];
        let mut out = Vec::new();
        put_batch(&batch, &rows, &mut out);
        let (read, numbers) = rows_of(&out).unwrap();
        assert_eq!(numbers, [30_u64, 10_u64, 0_u64]);
        assert_eq!(read.rows, 3_usize);
        assert!(matches!(&read.columns[0], Column::Ints(ints) if ints.scale == 2_u32 && ints.units[0] == i64::MIN));
        for (n, &(_, r, _)) in rows.iter().enumerate() {
            let made: Vec<Value> = (0..4).map(|at| read.cell(at, n).to_value()).collect();
            let was: Vec<Value> = (0..4).map(|at| batch.cell(at, r).to_value()).collect();
            assert_eq!(format!("{made:?}"), format!("{was:?}"), "row {r}");
        }
        let one = Batch { columns: vec![Column::Ints(Ints::new(vec![1_i64], 0_u32, None))], rows: 1 };
        put_batch(&one, &[(0_usize, 0_usize, 5_u64)], &mut out);
        assert!(matches!(&rows_of(&out).unwrap().0.columns[0], Column::Ints(ints) if ints.nulls.is_none()));
    }

    /// Verifies: [DESIGN] RULES.md D38, robustness: a record not as
    /// `put_batch` wrote it is refused with SQLCODE -916: cut short, of a
    /// count of rows its bytes cannot hold, of a column of no known tag, or
    /// with bytes left over; one of no known kind, or a piece cut short.
    #[test]
    fn records_not_of_a_batch_are_refused() {
        let mut out = Vec::new();
        put_batch(&batch(), &[(0_usize, 0_usize, 1_u64)], &mut out);
        let types = TypeRegistry::default();
        let code = |record: &[u8]| record_of(record, &types).err().map(|e| e.code);
        assert_eq!(code(&out[..out.len() - 1]), Some(Code::Corrupt), "cut short");
        assert_eq!(code(&[ROWS_TAG, 0xff_u8, 0xff_u8, 0xff_u8, 0x7f_u8]), Some(Code::Corrupt), "too many rows");
        assert_eq!(code(&[7_u8]), Some(Code::Corrupt), "no such record");
        assert_eq!(code(&[]), Some(Code::Corrupt), "empty");
        assert_eq!(code(&[PIECE_TAG, 1_u8, 0_u8, 0_u8, 0_u8, 1_u8]), Some(Code::Corrupt), "a piece cut short");
        let mut tagged = vec![ROWS_TAG];
        tagged.put_u32(0_u32);
        tagged.put_u32(1_u32);
        tagged.push(9_u8);
        assert_eq!(code(&tagged), Some(Code::Corrupt), "no such tag");
        assert_eq!(code(&[out.as_slice(), &[0_u8]].concat()), Some(Code::Corrupt), "bytes left over");
    }

    /// Verifies: [DESIGN] RULES.md D38, robustness: rows written to
    /// partitions never made are refused with SQLCODE -910, not lost; a
    /// pass's run is made the first time rows are written to it; a
    /// partition's rows of a batch are records of at most `RECORD_ROWS`.
    #[test]
    fn rows_are_written_where_they_belong() {
        let st = State::with_builtins(Box::new(MemoryCatalog::default()));
        let mut work = Work::new(&st);
        let batch = batch();
        let mut lost_rows = vec![(1_usize, 0_usize, 0_u64)];
        assert_eq!(
            Overflow::Partitions(None).write(&batch, (&mut lost_rows, &mut Queued::default()), &mut work).err().map(|e| e.code),
            Some(Code::Api)
        );
        let mut run = Overflow::Run(None);
        assert_eq!(run.part_of(b"k", &mut work).unwrap(), 0_usize);
        let mut passed = vec![(0_usize, 2_usize, 2_u64), (0_usize, 0_usize, 0_u64)];
        run.write(&batch, (&mut passed, &mut Queued::default()), &mut work).unwrap();
        assert_eq!(passed.len(), 0_usize, "emptied");
        let Overflow::Run(Some(made)) = run else { panic!("a run made") };
        let records: Vec<Vec<u8>> = made.into_records().unwrap().collect::<Result<_>>().unwrap();
        assert_eq!(records.len(), 1_usize);
        assert_eq!(rows_of(&records[0]).unwrap().1, [2_u64, 0_u64]);
        let mut parts = Overflow::Partitions(None);
        let at = parts.part_of(b"key", &mut work).unwrap();
        let mut many: Vec<(usize, usize, u64)> = (0..=RECORD_ROWS).map(|n| (at, 0_usize, u64::try_from(n).unwrap())).collect();
        parts.write(&batch, (&mut many, &mut Queued::default()), &mut work).unwrap();
        let Overflow::Partitions(Some(partitions)) = parts else { panic!("partitions made") };
        let written: Vec<usize> = partitions.into_runs().into_iter().map(|part| part.into_records().unwrap().count()).collect();
        assert_eq!(written.iter().sum::<usize>(), 2_usize, "{RECORD_ROWS} rows a record: {written:?}");
    }

    /// A plug-in's accumulator that writes no state.
    struct Stateless;

    impl Accumulator for Stateless {
        fn update(&mut self, _v: &Value) -> Result<()> {
            Ok(())
        }
        fn finish(&self) -> Result<Value> {
            Ok(Value::Null)
        }
    }

    /// Verifies: [DESIGN] RULES.md D38, D53: groups partly made, written
    /// as pieces of one record (`put_group`, `put_pieces`), read back in
    /// order, each as its first row's number, grouping values and set
    /// functions' states, each merged as it was; a group of a set function
    /// of no state is not written; a piece read back and written again
    /// (`put_piece`) is as it was.
    #[test]
    fn pieces_are_read_back_as_written() {
        let count = Registry::with_set_functions().aggregate("COUNT").unwrap().0;
        let counted = || count.accumulator(DataType::Exact { precision: 18_u32, scale: 0_u32, spelled: crate::value::Spelled::Integer });
        let (mut acc, other) = (counted(), counted());
        acc.update_rows(3_usize).unwrap();
        let values = vec![Value::Char("k".to_owned()), Value::Null];
        let mut body = Vec::new();
        assert!(put_group(&(41_u64, values.clone(), vec![acc]), &mut body));
        assert!(put_group(&(7_u64, vec![Value::Null, Value::Null], vec![other]), &mut body));
        let mut out = Vec::new();
        put_pieces(2_u32, &body, &mut out);
        let Record::Pieces(pieces) = record_of(&out, &TypeRegistry::default()).unwrap() else { panic!("pieces") };
        assert_eq!(pieces.iter().map(|p| p.first).collect::<Vec<_>>(), [41_u64, 7_u64], "in order");
        let read = &pieces[0];
        assert_eq!((read.first, format!("{:?}", read.values)), (41_u64, format!("{values:?}")));
        let mut merged = counted();
        merged.merge(&read.states[0]).unwrap();
        assert_eq!(merged.finish().unwrap().to_string(), "3");
        assert_eq!(merged.merge(&[1_u8]).err().map(|e| e.code), Some(Code::Corrupt), "not a count's state");
        let stateless: Box<dyn Accumulator> = Box::new(Stateless);
        assert!(!put_group(&(0_u64, Vec::new(), vec![stateless]), &mut out));
        let mut again = Vec::new();
        put_piece(read, &mut again);
        put_pieces(1_u32, &again, &mut out);
        let Record::Pieces(twice) = record_of(&out, &TypeRegistry::default()).unwrap() else { panic!("pieces") };
        assert_eq!((twice[0].first, &twice[0].states), (41_u64, &read.states), "written again as read");
    }
}
