//! \[DESIGN\] Byte records sorted bytewise, in a work area (RULES.md D38):
//! held in memory up to the area's bytes and sorted there; past that, each
//! area's worth sorted and written as a run of a spill area, the runs
//! merged, as many at once as the area reads, in passes until one is left.
//! For a plug-in's work that outgrows memory, as an index loaded whole
//! (`Index::load`); the executor's sort orders rows by SQL instead
//! (`exec/sort.rs`). A tie goes to the earlier record.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::{Area, Records, SpillArea, SpillRun, WorkAreas};
use crate::error::Result;

/// The bytes a record held in memory takes beyond its own: its vector.
const RECORD_BYTES: usize = 24;

/// `records`, sorted bytewise, in an area of `areas` as `area` asks: one
/// is made only if they outgrow its bytes.
///
/// # Errors
///
/// The records' own; the spill area's: SQLCODE -918 if the budget cannot
/// grant it.
pub fn sorted_records(areas: &WorkAreas, area: Area, records: &mut dyn Iterator<Item = Result<Vec<u8>>>) -> Result<Records> {
    let (mut held, mut bytes, mut runs) = (Vec::new(), 0_usize, Vec::new());
    let mut spill: Option<Box<dyn SpillArea>> = None;
    for record in records {
        let record = record?;
        bytes = bytes.saturating_add(record.len()).saturating_add(RECORD_BYTES);
        held.push(record);
        if bytes > area.bytes {
            let made = match spill.take() {
                Some(made) => made,
                None => areas.area(area)?,
            };
            runs.push(written(&*made, std::mem::take(&mut held))?);
            spill = Some(made);
            bytes = 0;
        }
    }
    let Some(spill) = spill else {
        held.sort();
        return Ok(Box::new(held.into_iter().map(Ok)));
    };
    if !held.is_empty() {
        runs.push(written(&*spill, held)?);
    }
    // Each run read or written holds a batch, and its records.
    let width = area.bytes.checked_div(area.batch.saturating_mul(2)).unwrap_or_default().max(2);
    while runs.len() > width {
        let (mut merged, mut rest) = (Vec::new(), runs.into_iter());
        loop {
            let group: Vec<Box<dyn SpillRun>> = rest.by_ref().take(width).collect();
            if group.is_empty() {
                break;
            }
            let mut run = spill.run()?;
            for record in merge(group, None)? {
                run.push(&record?)?;
            }
            merged.push(run);
        }
        runs = merged;
    }
    Ok(Box::new(merge(runs, Some(spill))?))
}

/// `records`, sorted, written as a run of `spill`.
fn written(spill: &dyn SpillArea, mut records: Vec<Vec<u8>>) -> Result<Box<dyn SpillRun>> {
    records.sort();
    let mut run = spill.run()?;
    for record in &records {
        run.push(record)?;
    }
    Ok(run)
}

/// The records of sorted `runs`, merged in order, each run freed as it is
/// read; `spill`, the area they are in, kept until the merge is done.
fn merge(runs: Vec<Box<dyn SpillRun>>, spill: Option<Box<dyn SpillArea>>) -> Result<Merge> {
    let mut sources = Vec::new();
    for run in runs {
        sources.push(run.into_records()?);
    }
    let mut merge = Merge { sources, heads: BinaryHeap::new(), _spill: spill };
    for source in 0..merge.sources.len() {
        merge.refill(source)?;
    }
    Ok(merge)
}

/// Runs being merged: each one's next record in the heap, the least first,
/// of the earliest run among equal ones.
struct Merge {
    sources: Vec<Records>,
    heads: BinaryHeap<Reverse<(Vec<u8>, usize)>>,
    _spill: Option<Box<dyn SpillArea>>,
}

impl Merge {
    /// Puts the next record of `source`, if any, in the heap.
    fn refill(&mut self, source: usize) -> Result<()> {
        let Some(next) = self.sources.get_mut(source).and_then(Iterator::next) else { return Ok(()) };
        self.heads.push(Reverse((next?, source)));
        Ok(())
    }
}

impl Iterator for Merge {
    type Item = Result<Vec<u8>>;

    fn next(&mut self) -> Option<Result<Vec<u8>>> {
        let Reverse((record, source)) = self.heads.pop()?;
        Some(self.refill(source).map(|()| record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spill::testing::{Held, areas};

    /// Records that do not sort as given: a few hundred, with ties.
    fn shuffled() -> Vec<Vec<u8>> {
        (0..600_u32).map(|n| (n.wrapping_mul(7919) % 450).to_be_bytes().to_vec()).collect()
    }

    fn sorted(area: Area, spill: Option<std::sync::Arc<dyn crate::spill::Spill>>) -> Result<Vec<Vec<u8>>> {
        let areas = areas(spill, 1 << 20);
        sorted_records(&areas, area, &mut shuffled().into_iter().map(Ok))?.collect()
    }

    /// Verifies: [DESIGN] RULES.md D38: records that fit the area are
    /// sorted in memory; more are sorted in runs and merged, in several
    /// passes when the area reads fewer runs than there are; either way
    /// bytewise, ties kept; a spill area that fails, or records that do,
    /// fail the sort.
    #[test]
    fn records_sort_in_memory_or_in_runs() {
        let mut expected = shuffled();
        expected.sort();
        assert_eq!(sorted(Area::new(1 << 20, 4096), None).unwrap(), expected, "in memory");
        let held = || -> Option<std::sync::Arc<dyn crate::spill::Spill>> { Some(std::sync::Arc::new(Held { broken: false })) };
        assert_eq!(sorted(Area::new(512, 64), held()).unwrap(), expected, "runs of a few, merged four at a time");
        assert_eq!(sorted(Area::new(512, 1024), held()).unwrap(), expected, "two at a time");
        let broken: Option<std::sync::Arc<dyn crate::spill::Spill>> = Some(std::sync::Arc::new(Held { broken: true }));
        assert!(sorted(Area::new(512, 64), broken).is_err());
        let areas = areas(None, 1 << 20);
        let mut failing = [Ok(vec![1]), Err(crate::error::SqlError::new(crate::error::Code::Io, "no"))].into_iter();
        assert!(sorted_records(&areas, Area::new(1 << 20, 4096), &mut failing).is_err());
    }
}
