//! \[DESIGN\] (RULES.md D1133, D1228) What a second grouping that crosses
//! the first's partitions keeps of them. Each partition's thread gives its
//! groups of the second, each with its first and its states written, and
//! they are held, charged to a work grant of the second grouping's own,
//! for the statement's thread to merge. Once they pass the grant, those
//! held and every partition's after go to the spill instead, each to one
//! of 64 runs by the hash of its key, so the groups of one key share a
//! run. At the end each run is read back and merged alone, its groups
//! finished, kept as the HAVING condition keeps them, and written to a run
//! of their own in the order of their firsts, which the statement's
//! thread merges as it reads them (`grouped.rs`, `Ordered`).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use super::super::super::Limit;
use super::super::super::probe::{PARTS, part_of};
use super::super::waves::given_of;
use super::super::{Given, Merging, finished, fresh};
use crate::error::Result;
use crate::exec::group::{Spec, group_key};
use crate::exec::partition::{numbered, unnumbered};
use crate::exec::work::Work;
use crate::exec::{Db, Runner, Seed, broken};
use crate::logical::Expr;
use crate::spill::{SpillRun, row_bytes};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::{Binary, Value};

/// A group of a partition: its first, its grouping values and each set
/// function's state written.
pub(super) type Written = (u64, Row, Vec<Vec<u8>>);

/// The groups of the second grouping merged: those held in memory, in
/// the order of their firsts, and runs of them read back from the spill.
pub(super) type Merged = (Vec<Vec<(u64, Row)>>, Vec<Given>);

/// A second grouping crossing the first's partitions: the projections
/// between the two, the nearest the first last; whether a projection
/// failed in a partition; and the groups the partitions gave.
pub(super) struct Crossing {
    pub(super) steps: Vec<Vec<Expr>>,
    pub(super) failed: AtomicBool,
    kept: Mutex<Kept>,
}

/// The groups the partitions gave: held, with the bytes they hold of the
/// grant, made when the first are given; or in the spill once they
/// passed it.
#[derive(Default)]
struct Kept {
    groups: Vec<Written>,
    bytes: usize,
    limit: Option<Limit>,
    spilled: Option<Spill>,
}

/// The spill of the groups: its work area, and the run of each part of
/// their keys' hashes.
struct Spill {
    work: Work,
    runs: Vec<Option<Box<dyn SpillRun>>>,
}

impl Crossing {
    /// None given yet, of the projections `steps`.
    pub(super) fn new(steps: Vec<Vec<Expr>>) -> Crossing {
        Crossing { steps, failed: AtomicBool::new(false), kept: Mutex::new(Kept::default()) }
    }

    /// Keeps `groups`, a partition's of the grouping `spec`, made by
    /// `runner`: held while the grant holds them, else in the spill.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the spill area cannot be reserved; the area's
    /// errors.
    pub(super) fn add(&self, spec: &Spec, groups: Vec<Written>, runner: &Runner) -> Result<()> {
        if groups.is_empty() {
            return Ok(());
        }
        let (db, tx) = runner.running();
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.spilled.is_none() {
            let bytes = groups.iter().map(written_bytes).fold(0, usize::saturating_add);
            let limit = kept.limit.get_or_insert_with(|| Limit::of(db)).clone();
            if limit.take(bytes) {
                kept.bytes = kept.bytes.saturating_add(bytes);
                kept.groups.extend(groups);
                return Ok(());
            }
            limit.give(bytes.saturating_add(kept.bytes));
            kept.bytes = 0;
            let held = std::mem::take(&mut kept.groups);
            kept.spilled = Some(Spill::of(held, spec, (db, tx, &limit))?);
        }
        let Some(spill) = kept.spilled.as_mut() else { return Err(broken("a crossing grouping's spill is missing")) };
        let seed = db.functions.seed().clone();
        groups.into_iter().try_for_each(|group| spill.write(spec, group, &seed))
    }

    /// [X3.135-1986 §5.22 GR2] Its groups of all, each partition's states
    /// of a group merged, numbered by the least first, those `keep` keeps
    /// of them, in that order; none if a projection failed.
    ///
    /// # Errors
    ///
    /// As merging a state, reading the spill back, or `keep` fails.
    pub(super) fn merged(&self, spec: &Spec, mut keep: impl FnMut(Vec<(u64, Row)>) -> Result<Vec<(u64, Row)>>) -> Result<Option<Merged>> {
        if self.failed.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        let kept = std::mem::take(&mut *self.kept.lock().unwrap_or_else(PoisonError::into_inner));
        let Some(mut spill) = kept.spilled else {
            let groups = merged_groups(spec, kept.groups.into_iter().map(Ok))?;
            return Ok(Some((vec![keep(groups)?], Vec::new())));
        };
        let (types, lobs) = (spill.work.types().clone(), spill.work.lobs().clone());
        let mut done = Vec::new();
        for run in std::mem::take(&mut spill.runs).into_iter().flatten() {
            let read = run.into_records()?.map(|record| record.and_then(|r| read_back(spec, &unnumbered(&r, &types, &lobs)?)));
            let groups = keep(merged_groups(spec, read)?)?;
            if groups.is_empty() {
                continue;
            }
            let mut out = spill.work.run()?;
            for (n, row) in groups {
                out.push(&numbered(n, &row, &lobs))?;
            }
            done.push(out);
        }
        let held: Arc<Mutex<Work>> = Arc::new(Mutex::new(spill.work));
        let given = done.into_iter().map(|run| Ok(given_of(run.into_records()?, (types.clone(), lobs.clone()), held.clone())));
        Ok(Some((Vec::new(), given.collect::<Result<Vec<_>>>()?)))
    }
}

impl Spill {
    /// A spill of the area of `limit`'s grant, of the grouping `spec` of
    /// `db` run by `tx`, `held` written to it.
    ///
    /// # Errors
    ///
    /// As `write`.
    fn of(held: Vec<Written>, spec: &Spec, (db, tx, limit): (&Db, TxId, &Limit)) -> Result<Spill> {
        let mut spill = Spill { work: Work::of_grant(db, tx, Arc::clone(&limit.grant)), runs: (0..PARTS).map(|_| None).collect() };
        let seed = db.functions.seed().clone();
        held.into_iter().try_for_each(|group| spill.write(spec, group, &seed))?;
        Ok(spill)
    }

    /// Writes `group` to the run of the part of its key's hash by `seed`:
    /// its grouping values, then each state as a binary string.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the area cannot be reserved; the area's errors.
    fn write(&mut self, spec: &Spec, (first, values, states): Written, seed: &Seed) -> Result<()> {
        let p = part_of(&group_key(spec, &values)?, seed);
        let mut row = values;
        row.extend(states.into_iter().map(|state| Value::Binary(Binary::new(state))));
        let record = numbered(first, &row, self.work.lobs());
        let slot = self.runs.get_mut(p).ok_or_else(|| broken("a part of a crossing grouping's spill is missing"))?;
        let run = match slot {
            Some(run) => run,
            None => slot.insert(self.work.run()?),
        };
        run.push(&record)
    }
}

/// The bytes `group` holds: its first, its values and its states.
fn written_bytes(group: &Written) -> usize {
    let states = group.2.iter().map(Vec::len).fold(0, usize::saturating_add);
    row_bytes(&group.1).saturating_add(states).saturating_add(size_of::<u64>())
}

/// The group `Spill::write` wrote, of the grouping `spec`, read back with
/// its number.
///
/// # Errors
///
/// SQLCODE -910 of a state that is no binary string.
fn read_back(spec: &Spec, (first, row): &(u64, Row)) -> Result<Written> {
    let (values, states) = row.split_at_checked(spec.groups.len()).ok_or_else(|| broken("a crossing group's record is short"))?;
    let states = states.iter().map(|state| {
        if let Value::Binary(b) = state { Ok(b.bytes().to_vec()) } else { Err(broken("a crossing group's state is no binary string")) }
    });
    Ok((*first, values.to_vec(), states.collect::<Result<_>>()?))
}

/// The groups of `groups` of the grouping `spec`, each key's states
/// merged, numbered by the least first, finished, in that order.
///
/// # Errors
///
/// As reading a group, or merging a state, fails.
fn merged_groups(spec: &Spec, groups: impl Iterator<Item = Result<Written>>) -> Result<Vec<(u64, Row)>> {
    let mut found: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut made: Vec<Merging> = Vec::new();
    for group in groups {
        let (number, values, states) = group?;
        let at = *found.entry(group_key(spec, &values)?).or_insert_with(|| {
            made.push((number, values, fresh(&spec.aggs)));
            made.len().saturating_sub(1)
        });
        let merging = made.get_mut(at).ok_or_else(|| broken("a merged group is missing"))?;
        merging.0 = merging.0.min(number);
        for (acc, state) in merging.2.iter_mut().zip(&states) {
            acc.merge(state)?;
        }
    }
    let mut out = made.into_iter().map(|(first, values, states)| Ok((first, finished(states, values)?))).collect::<Result<Vec<_>>>()?;
    out.sort_unstable_by_key(|g| g.0);
    Ok(out)
}

// Unit tests: groups merged after passing the grant into the spill are
// those merged in memory (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
