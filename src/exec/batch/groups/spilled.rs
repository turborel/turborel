//! \[DESIGN\] RULES.md D31, D38: the rows of groups a grouping by batch has
//! no room for, written by batch. Of each batch, the rows of each
//! partition, by a hash of their keys (`partition.rs`), are one record
//! (at most `RECORD_ROWS` rows a record): their numbers, then each column's
//! values, of a vector of integers its units, scale and nulls, of any
//! other its values (`codec::put_value`). So a partition is read back as
//! batches of the same columns, integers as integers, and grouped again
//! by batch (`groups.rs`), its keys made as before; one that outgrows the
//! work area is partitioned again, as many times as SPILL.PARTITION_DEPTH
//! says, then made in passes, each reading again the rows the last had no
//! room for. A plug-in's accumulator is opaque, so its state is never
//! written: a row is written whole, its group made later. Each group is
//! numbered by its first row, and the groups merged by number, as
//! `exec/group.rs` merges them, so they come in the order their first rows
//! came, as in memory.

use super::{Grouping, Groups, Numbers};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::exec::RowIter;
use crate::exec::group::Finished;
use crate::exec::partition::{Partitioner, merge_all, numbered};
use crate::exec::work::Work;
use crate::spill::{Records, SpillRun};
use crate::storage::{Batch, Column, Ints};
use crate::value::Value;
use crate::value::codec::{put_value, value};
use crate::value::types::TypeRegistry;

/// The most rows a record holds, so a batch's rows of one partition, or of
/// a pass, are a few records of a run's batch of bytes, not one far larger.
const RECORD_ROWS: usize = 256;

/// The tag of a column written as values, and of one written as integers.
const VALUES: u8 = 0;
const INTEGERS: u8 = 1;

/// The bytes of a row's number, a `u64`, the least a record holds of each
/// of its rows: a count of rows past its bytes is not believed.
const NUMBER_BYTES: usize = 8;

/// Where a grouping writes the rows of groups it has no room for: to
/// partitions, or to one run, for the next pass; each made when first
/// needed.
pub(super) enum Overflow {
    Partitions(Option<Partitioner>),
    Run(Option<Box<dyn SpillRun>>),
}

impl Overflow {
    /// The place of the partition of `key`, of those made in `work` the
    /// first time; 0 for a run.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn part_of(&mut self, key: &[u8], work: &mut Work) -> Result<usize> {
        match self {
            Overflow::Partitions(parts) => {
                let parts = match parts {
                    Some(parts) => parts,
                    None => parts.insert(Partitioner::of(work)?),
                };
                Ok(parts.place_of(key))
            }
            Overflow::Run(_) => Ok(0),
        }
    }

    /// Writes the rows `pending` of `batch`, each its partition, place and
    /// number, a record of each partition's at a time, in `work`, in the
    /// order they came; and empties it.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn write(&mut self, batch: &Batch, pending: &mut Vec<(usize, usize, u64)>, work: &mut Work) -> Result<()> {
        // Stable: each partition's rows keep their order.
        pending.sort_by_key(|&(part, ..)| part);
        let mut out = Vec::new();
        for rows in pending.chunk_by(|a, b| a.0 == b.0).flat_map(|part| part.chunks(RECORD_ROWS)) {
            let Some(&(part, ..)) = rows.first() else { continue };
            put_batch(batch, rows, &mut out);
            match self {
                Overflow::Partitions(Some(parts)) => parts.push_at(part, &out)?,
                Overflow::Run(made) => {
                    let run = match made {
                        Some(run) => run,
                        None => made.insert(work.run()?),
                    };
                    run.push(&out)?;
                }
                Overflow::Partitions(None) => return Err(lost()),
            }
        }
        pending.clear();
        Ok(())
    }
}

/// The groups: `held`, each numbered by its first row, in that order, and
/// those of the rows written to `overflow`, grouped as `how` says, in
/// `work`, merged by number.
///
/// # Errors
///
/// The spill area's; the set functions'.
pub(super) fn merged(held: Finished, overflow: Overflow, how: Grouping<'_>, work: &mut Work) -> Result<RowIter> {
    let Overflow::Partitions(Some(parts)) = overflow else { return Ok(Box::new(held.into_iter().map(|(_, row)| Ok(row)))) };
    let mut sources = vec![written(held, work)?];
    for run in parts.into_runs() {
        regrouped(run.into_records()?, (0, &mut sources), how, work)?;
    }
    Ok(Box::new(merge_all(sources, work)?.map(|r| r.map(|(_, row)| row))))
}

/// The groups of the rows of the records `partition`, partitioned `depth`
/// times again before, each pass's groups a run added to `sources`.
fn regrouped(partition: Records, (depth, sources): (usize, &mut Vec<Records>), how: Grouping<'_>, work: &mut Work) -> Result<()> {
    let mut next = Some(partition);
    while let Some(records) = next.take() {
        let to = if depth < work.depth() { Overflow::Partitions(None) } else { Overflow::Run(None) };
        let mut groups = Groups::new(work.limit(), to);
        let types = work.types().clone();
        for record in records {
            let (batch, numbers) = batch_of(&record?, &types)?;
            groups.feed((&batch, None), Numbers::Given(&numbers), how, work)?;
        }
        let (held, written_to) = groups.finished()?;
        sources.push(written(held, work)?);
        match written_to {
            Overflow::Partitions(Some(parts)) => {
                for run in parts.into_runs() {
                    regrouped(run.into_records()?, (depth.saturating_add(1), &mut *sources), how, work)?;
                }
            }
            Overflow::Run(Some(run)) => next = Some(run.into_records()?),
            Overflow::Partitions(None) | Overflow::Run(None) => {}
        }
    }
    Ok(())
}

/// Groups `held` in a run of `work`, read from the first.
fn written(held: Finished, work: &mut Work) -> Result<Records> {
    let mut run = work.run()?;
    for (n, row) in held {
        run.push(&numbered(n, &row))?;
    }
    run.into_records()
}

/// Writes to `out`, emptied first, the rows `rows` of `batch`, each its
/// partition, place and number, as a record: their count and numbers,
/// then each column's tag and values.
fn put_batch(batch: &Batch, rows: &[(usize, usize, u64)], out: &mut Vec<u8>) {
    out.clear();
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
                Column::Ints(_) | Column::Floats(_) | Column::Texts(_) => put_value(out, &batch.cell(at, r).to_value()),
            }
        }
    }
}

/// The batch a record `put_batch` wrote holds, and its rows' numbers, its
/// values of types beyond the standard found among `types`.
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
fn malformed() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a spilled batch's record is malformed")
}

/// The error of partitions written before they were made.
fn lost() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a grouping's partitions were not made")
}

// Unit tests: records of batches written and read back, and records not
// so (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MemoryCatalog;
    use crate::db::State;
    use crate::storage::{Floats, Texts};
    use crate::value::{Bits, Decimal};

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
        let (read, numbers) = batch_of(&out, &TypeRegistry::default()).unwrap();
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
        assert!(matches!(&batch_of(&out, &TypeRegistry::default()).unwrap().0.columns[0], Column::Ints(ints) if ints.nulls.is_none()));
    }

    /// Verifies: [DESIGN] RULES.md D38, robustness: a record not as
    /// `put_batch` wrote it is refused with SQLCODE -916: cut short, of a
    /// count of rows its bytes cannot hold, of a column of no known tag, or
    /// with bytes left over.
    #[test]
    fn records_not_of_a_batch_are_refused() {
        let mut out = Vec::new();
        put_batch(&batch(), &[(0_usize, 0_usize, 1_u64)], &mut out);
        let types = TypeRegistry::default();
        let code = |record: &[u8]| batch_of(record, &types).err().map(|e| e.code);
        assert_eq!(code(&out[..out.len() - 1]), Some(Code::Corrupt), "cut short");
        assert_eq!(code(&[0xff_u8, 0xff_u8, 0xff_u8, 0x7f_u8]), Some(Code::Corrupt), "too many rows");
        let mut tagged = Vec::new();
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
        assert_eq!(Overflow::Partitions(None).write(&batch, &mut lost_rows, &mut work).err().map(|e| e.code), Some(Code::Api));
        let mut run = Overflow::Run(None);
        assert_eq!(run.part_of(b"k", &mut work).unwrap(), 0_usize);
        let mut passed = vec![(0_usize, 2_usize, 2_u64), (0_usize, 0_usize, 0_u64)];
        run.write(&batch, &mut passed, &mut work).unwrap();
        assert_eq!(passed.len(), 0_usize, "emptied");
        let Overflow::Run(Some(made)) = run else { panic!("a run made") };
        let records: Vec<Vec<u8>> = made.into_records().unwrap().collect::<Result<_>>().unwrap();
        assert_eq!(records.len(), 1_usize);
        assert_eq!(batch_of(&records[0], &TypeRegistry::default()).unwrap().1, [2_u64, 0_u64]);
        let mut parts = Overflow::Partitions(None);
        let at = parts.part_of(b"key", &mut work).unwrap();
        let mut many: Vec<(usize, usize, u64)> = (0..=RECORD_ROWS).map(|n| (at, 0_usize, u64::try_from(n).unwrap())).collect();
        parts.write(&batch, &mut many, &mut work).unwrap();
        let Overflow::Partitions(Some(partitions)) = parts else { panic!("partitions made") };
        let written: Vec<usize> = partitions.into_runs().into_iter().map(|part| part.into_records().unwrap().count()).collect();
        assert_eq!(written.iter().sum::<usize>(), 2_usize, "{RECORD_ROWS} rows a record: {written:?}");
    }
}
