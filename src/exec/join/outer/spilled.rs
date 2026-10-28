//! \[DESIGN\] An outer join on keys whose right operand outgrows its work
//! area (RULES.md D38), partitioned as the hash join is
//! (`join/partitioned.rs`): both operands' rows numbered, in the order they
//! came, and written to runs by a hash of their key values; each pair of
//! partitions joined, its right partition hashed in memory, or in a table
//! of the spill area if it outgrows the work area too, with a flag for each
//! of its rows, whether it matched.
//!
//! The outer join's rows come in its order [ISO/IEC 9075:1992 §7.5]: the
//! left operand's, each row with its matches in the right's order, or with
//! nulls; then the right's that matched none. The partitions' rows of
//! each are merged by number. A row with a null key matches nothing
//! [X3.135-1986 §5.11 GR2]: it goes straight to the rows of a preserved
//! side. Split from exec/join/outer.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};

use super::super::super::partition::{merge_all, numbered, unnumbered};
use super::super::super::work::Work;
use super::super::super::{RowIter, failed};
use super::super::key;
use super::Spec;
use crate::error::{Code, Result, SqlError};
use crate::spill::{Records, SpillRun, SpillTable, bucket_key, row_bytes};
use crate::storage::Row;
use crate::value::Bucket;

/// The outer join `spec` says of `left` with a right operand of which
/// `held` came first, then `rest`, in `work`. Never inlined into the
/// caller, whose frame is on the stack while the left operand is made
/// (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn joined(left: RowIter, (held, rest): (Vec<Row>, RowIter), spec: &Spec, mut work: Work) -> RowIter {
    match partitioned(left, (held, rest), spec, &mut work) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// `joined`'s work.
fn partitioned(left: RowIter, (held, rest): (Vec<Row>, RowIter), spec: &Spec, work: &mut Work) -> Result<RowIter> {
    let parts = Parts::new(spec, work)?;
    let (rights, lonely_right) = parts.rights(held.into_iter().map(Ok).chain(rest), spec, work)?;
    let (lefts, lonely_left) = parts.lefts(left, spec, work)?;
    let (mut from_left, mut from_right) = (vec![lonely_left.into_records()?], vec![lonely_right.into_records()?]);
    for ((right, bytes), probes) in rights.into_iter().zip(lefts) {
        let mut part = Part::of(right, (bytes, &spec.keys), work)?;
        let mut matched = vec![false; part.count];
        from_left.push(part.probed(probes.into_records()?, (spec, &mut matched), work)?);
        if spec.kind.keeps_right() {
            from_right.push(part.unmatched(&matched, spec, work)?);
        }
    }
    let left_rows = merge_all(from_left, work)?;
    let right_rows = merge_all(from_right, work)?;
    Ok(Box::new(left_rows.chain(right_rows).map(|r| r.map(|(_, row)| row))))
}

/// How an outer join's rows are partitioned: by a hash of their key
/// values, into as many partitions as the work area writes at once.
struct Parts {
    hasher: RandomState,
    count: usize,
}

impl Parts {
    fn new(spec: &Spec, work: &Work) -> Result<Parts> {
        if spec.keys.is_empty() {
            return Err(lost());
        }
        Ok(Parts { hasher: RandomState::new(), count: work.width().min(work.partitions()) })
    }

    /// The partition of key values `b`.
    fn of(&self, b: &[Bucket]) -> usize {
        let n = u64::try_from(self.count).unwrap_or(1);
        usize::try_from(self.hasher.hash_one(bucket_key(b)).checked_rem(n).unwrap_or_default()).unwrap_or_default()
    }

    /// `count` new runs of `work`.
    fn runs(&self, work: &mut Work) -> Result<Vec<Box<dyn SpillRun>>> {
        (0..self.count).map(|_| work.run()).collect()
    }

    /// The right operand `rows`, numbered, in partitions, each with the
    /// bytes its rows take in memory; and, of a preserved right, its rows
    /// with a null key, after nulls.
    fn rights(&self, rows: impl Iterator<Item = Result<Row>>, spec: &Spec, work: &mut Work) -> Result<Partitioned> {
        let mut runs: Vec<(Box<dyn SpillRun>, usize)> = self.runs(work)?.into_iter().map(|run| (run, 0)).collect();
        let mut lonely = work.run()?;
        for (n, r) in (0_u64..).zip(rows) {
            let row = r?;
            match key(&row, spec.keys.iter().map(|k| k.1))? {
                Some(b) => {
                    let (run, bytes) = runs.get_mut(self.of(&b)).ok_or_else(lost)?;
                    *bytes = bytes.saturating_add(row_bytes(&row));
                    run.push(&numbered(n, &row))?;
                }
                None => {
                    if spec.kind.keeps_right() {
                        lonely.push(&numbered(n, &spec.right_alone(&row)))?;
                    }
                }
            }
        }
        Ok((runs, lonely))
    }

    /// The left operand `rows`, numbered, in partitions; and, of a
    /// preserved left, its rows with a null key, with nulls.
    fn lefts(&self, rows: RowIter, spec: &Spec, work: &mut Work) -> Result<Lefts> {
        let mut runs = self.runs(work)?;
        let mut lonely = work.run()?;
        for (n, r) in (0_u64..).zip(rows) {
            let row = r?;
            match key(&row, spec.keys.iter().map(|k| k.0))? {
                Some(b) => runs.get_mut(self.of(&b)).ok_or_else(lost)?.push(&numbered(n, &row))?,
                None => {
                    if spec.kind.keeps_left() {
                        lonely.push(&numbered(n, &spec.left_alone(&row)))?;
                    }
                }
            }
        }
        Ok((runs, lonely))
    }
}

/// The right operand's partitions, each with its bytes, and its rows of a
/// null key.
type Partitioned = (Vec<(Box<dyn SpillRun>, usize)>, Box<dyn SpillRun>);

/// The left operand's partitions, and its rows of a null key.
type Lefts = (Vec<Box<dyn SpillRun>>, Box<dyn SpillRun>);

/// A right partition, built to be probed: its rows by bucket, in memory or
/// in a table of the spill area, each with its place in the partition and
/// its number in the operand; and the run it was read from, read again for
/// the rows that matched none.
struct Part {
    run: Box<dyn SpillRun>,
    count: usize,
    rows: Vec<(u64, Row)>,
    buckets: HashMap<Vec<Bucket>, Vec<usize>>,
    spilled: Option<Box<dyn SpillTable>>,
}

impl Part {
    /// The partition in `run`, of `bytes`, hashed on its columns of `keys`:
    /// in a table of `work`'s spill area if it outgrows it.
    fn of(mut run: Box<dyn SpillRun>, (bytes, keys): (usize, &[(usize, usize)]), work: &mut Work) -> Result<Part> {
        let spilled = if bytes > work.limit() { Some(work.table()?) } else { None };
        let mut part = Part { run: work.run()?, count: 0, rows: Vec::new(), buckets: HashMap::new(), spilled };
        for record in run.records()? {
            let record = record?;
            let (n, row) = unnumbered(&record)?;
            let b = key(&row, keys.iter().map(|k| k.1))?.ok_or_else(lost)?;
            if let Some(table) = &mut part.spilled {
                table.insert(&bucket_key(&b), &placed(part.count, &record))?;
            } else {
                part.buckets.entry(b).or_default().push(part.rows.len());
                part.rows.push((n, row));
            }
            part.count = part.count.saturating_add(1);
        }
        part.run = run;
        Ok(part)
    }

    /// The rows of the bucket of left row `l`'s key, each with its place.
    fn candidates(&mut self, l: &Row, keys: &[(usize, usize)]) -> Result<Vec<(usize, Row)>> {
        let Some(b) = key(l, keys.iter().map(|k| k.0))? else { return Ok(Vec::new()) };
        if let Some(table) = &self.spilled {
            return table.get(&bucket_key(&b))?.iter().map(|v| unplaced(v).map(|(at, (_, row))| (at, row))).collect();
        }
        let found = self.buckets.get(&b).map(Vec::as_slice).unwrap_or_default();
        Ok(found.iter().filter_map(|&at| self.rows.get(at).map(|(_, row)| (at, row.clone()))).collect())
    }

    /// The rows `probes`, numbered left rows of the partition, each with
    /// the rows of its that match it, marked in `matched`, or with nulls if
    /// none does and the left is preserved; numbered, in a run of `work`,
    /// read from the first.
    fn probed(&mut self, probes: Records, (spec, matched): (&Spec, &mut [bool]), work: &mut Work) -> Result<Records> {
        let mut out = work.run()?;
        for record in probes {
            let (n, l) = unnumbered(&record?)?;
            let mut found = false;
            for (at, r) in self.candidates(&l, &spec.keys)? {
                let Some(joined) = spec.joined(&l, &r)? else { continue };
                found = true;
                if let Some(m) = matched.get_mut(at) {
                    *m = true;
                }
                out.push(&numbered(n, &joined))?;
            }
            if !found && spec.kind.keeps_left() {
                out.push(&numbered(n, &spec.left_alone(&l)))?;
            }
        }
        out.into_records()
    }

    /// Its rows `matched` does not mark, after nulls, each with its number,
    /// in a run of `work`, read from the first.
    fn unmatched(self, matched: &[bool], spec: &Spec, work: &mut Work) -> Result<Records> {
        let mut out = work.run()?;
        for (at, record) in self.run.into_records()?.enumerate() {
            let (n, row) = unnumbered(&record?)?;
            if !matched.get(at).copied().unwrap_or(true) {
                out.push(&numbered(n, &spec.right_alone(&row)))?;
            }
        }
        out.into_records()
    }
}

/// A numbered record with its place in its partition before it.
fn placed(at: usize, record: &[u8]) -> Vec<u8> {
    [&u64::try_from(at).unwrap_or(u64::MAX).to_le_bytes()[..], record].concat()
}

/// The place, number and row of a record `placed` made.
fn unplaced(value: &[u8]) -> Result<(usize, (u64, Row))> {
    let (at, record) = value.split_at_checked(8).ok_or_else(lost)?;
    let at = <[u8; 8]>::try_from(at).map(u64::from_le_bytes).map_err(|_| lost())?;
    Ok((usize::try_from(at).unwrap_or(usize::MAX), unnumbered(record)?))
}

/// The error of a partition not as it was written.
fn lost() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] an outer join's partition is not as it was written")
}
