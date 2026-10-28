//! [X3.135-1986 §5.25 GR2, GR5, §5.11 GR7] DISTINCT: each row the first
//! time it comes, duplicates per the comparison rules, nulls identical.
//! \[DESIGN\] RULES.md D38: the rows seen are held in memory, and each row
//! given as it comes, while their keys fit the work area; once they do not,
//! the rest of the rows not among them are partitioned by key, numbered,
//! each partition's first of each key kept, a partition that outgrows the
//! work area partitioned again, and those merged back by number
//! (`partition.rs`), so the rows come in the order they would in memory.
//! Split from exec.rs (RULES.md R-SIZE-2).

use std::collections::HashSet;

use super::partition::{Partitioner, merge_all, numbered, unnumbered};
use super::work::{Bag, ENTRY_BYTES, Work};
use super::{RowIter, failed};
use crate::db::State;
use crate::error::Result;
use crate::spill::{Records, norm_key};
use crate::storage::Row;
use crate::value::Value;

/// The rows of `input`, DISTINCT. Never inlined into the caller, whose
/// frame is on the stack while its input is made (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn distinct_of(input: RowIter, db: &State) -> RowIter {
    Box::new(Distinct { input, seen: HashSet::new(), bytes: 0, work: Work::new(db), rest: None })
}

/// A row as DISTINCT knows it.
fn key(row: &Row) -> Vec<u8> {
    norm_key(&row.iter().map(Value::norm).collect::<Vec<_>>())
}

/// DISTINCT, as it is pulled: the keys seen, and their bytes, while they
/// fit `work`; once they do not, the rows of the partitions, merged.
struct Distinct {
    input: RowIter,
    seen: HashSet<Vec<u8>>,
    bytes: usize,
    work: Work,
    rest: Option<RowIter>,
}

impl Distinct {
    /// The next row not seen before, if there is one. Never inlined into
    /// the iterator, as a row is pulled through every level of a plan on
    /// the stack (RULES.md R-CF-6).
    #[inline(never)]
    fn step(&mut self) -> Option<Result<Row>> {
        loop {
            if let Some(rest) = &mut self.rest {
                return rest.next();
            }
            let row = match self.input.next()? {
                Ok(row) => row,
                Err(e) => return Some(Err(e)),
            };
            let k = key(&row);
            if self.seen.contains(&k) {
                continue;
            }
            self.bytes = self.bytes.saturating_add(k.len()).saturating_add(ENTRY_BYTES);
            self.seen.insert(k);
            if self.bytes > self.work.limit() {
                self.rest = Some(self.partitioned().unwrap_or_else(failed));
            }
            return Some(Ok(row));
        }
    }

    /// The rest of the input's rows, DISTINCT and not among those seen:
    /// partitioned by key, numbered, each partition's first of each key
    /// kept, and merged back by number.
    fn partitioned(&mut self) -> Result<RowIter> {
        let mut parts = Partitioner::of(&mut self.work)?;
        let input = std::mem::replace(&mut self.input, Box::new(std::iter::empty()));
        for (n, r) in (0_u64..).zip(input) {
            let row = r?;
            let k = key(&row);
            if !self.seen.contains(&k) {
                parts.push(&k, &numbered(n, &row))?;
            }
        }
        self.seen = HashSet::new();
        let mut sources = Vec::new();
        for run in parts.into_runs() {
            firsts(run.into_records()?, (0, &mut sources), &mut self.work)?;
        }
        Ok(Box::new(merge_all(sources, &mut self.work)?.map(|r| r.map(|(_, row)| row))))
    }
}

/// The first of each key of the numbered rows `partition`, partitioned
/// `depth` times again before, each in a run added to `sources`, in order
/// of number: those held while they fit `work`; then, of the rest, those
/// of each partition again, while SPILL.PARTITION_DEPTH allows, else those
/// a bag of the keys, which spills, holds first.
fn firsts(partition: Records, (depth, sources): (usize, &mut Vec<Records>), work: &mut Work) -> Result<()> {
    let (mut seen, mut bytes, mut out) = (HashSet::new(), 0_usize, work.run()?);
    let (again, mut parts, mut bag) = (depth < work.depth(), None, None);
    for record in partition {
        let record = record?;
        let k = key(&unnumbered(&record, work.types())?.1);
        if seen.contains(&k) {
            continue;
        }
        if bytes <= work.limit() {
            bytes = bytes.saturating_add(k.len()).saturating_add(ENTRY_BYTES);
            seen.insert(k);
            out.push(&record)?;
        } else if again {
            let parts = match &mut parts {
                Some(parts) => parts,
                None => parts.insert(Partitioner::of(work)?),
            };
            parts.push(&k, &record)?;
        } else if bag.get_or_insert_with(|| Bag::new(work.another())).add_new(&k)? {
            out.push(&record)?;
        }
    }
    sources.push(out.into_records()?);
    drop(seen);
    for run in parts.map(Partitioner::into_runs).unwrap_or_default() {
        firsts(run.into_records()?, (depth.saturating_add(1), &mut *sources), work)?;
    }
    Ok(())
}

impl Iterator for Distinct {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step()
    }
}
