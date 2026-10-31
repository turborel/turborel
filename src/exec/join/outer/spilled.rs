//! \[DESIGN\] An outer join on keys whose right operand outgrows its work
//! area (RULES.md D38), partitioned as the hash join is
//! (`join/partitioned.rs`): both operands' rows numbered, in the order they
//! came, and written to runs by a hash of their key values; a right
//! partition that still outgrows the work area partitioned again, with its
//! left partition, by another hash, up to SPILL.PARTITION_DEPTH times; one
//! that does not shrink, as when one key holds most of its rows, or is past
//! that depth, joined a part of the work area's size at a time
//! (`outer/chunked.rs`). Each pair of partitions is joined with a flag for
//! each right row held, whether it matched.
//!
//! The outer join's rows come in its order [ISO/IEC 9075:1992 §7.5]: the
//! left operand's, each row with its matches in the right's order, or with
//! nulls; then the right's that matched none. The partitions' rows of
//! each are merged by number. A row with a null key matches nothing
//! [X3.135-1986 §5.11 GR2]: it goes straight to the rows of a preserved
//! side. Split from exec/join/outer.rs (RULES.md R-SIZE-2).

use std::hash::{BuildHasher, RandomState};

use super::super::super::partition::{merge_all, numbered, unnumbered};
use super::super::super::work::Work;
use super::super::super::{RowIter, failed};
use super::super::key;
use super::{PLACE_BYTES, Spec, chunked};
use crate::error::{Code, Result, SqlError};
use crate::logical::Cond;
use crate::spill::{Records, SpillRun, bucket_key, row_bytes};
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

/// The runs of numbered rows an outer join gives: of its left operand's
/// rows, each with its matches or nulls; and of its right's that matched
/// none, after nulls. Each merged by number.
pub(super) struct Sources {
    pub(super) left: Vec<Records>,
    pub(super) right: Vec<Records>,
}

/// `joined`'s work.
fn partitioned(left: RowIter, (held, rest): (Vec<Row>, RowIter), spec: &Spec, work: &mut Work) -> Result<RowIter> {
    let parts = Parts::new(spec, work)?;
    let (rights, lonely_right) = parts.rights(held.into_iter().map(Ok).chain(rest), spec, work)?;
    let (lefts, lonely_left, any_left) = parts.lefts(left, spec, work)?;
    // The right's own condition judged of each of its rows as the left has
    // one [ISO/IEC 9075:1992 §7.5 GR1c].
    let judged = spec.own.1.as_ref().filter(|_| any_left);
    let lonely_right = alone_rights(lonely_right.into_records()?, (spec, judged), work)?;
    let mut sources = Sources { left: vec![lonely_left.into_records()?], right: vec![lonely_right] };
    for ((right, bytes), probes) in rights.into_iter().zip(lefts) {
        pair((right, bytes, probes), (spec, judged, 0), &mut sources, work)?;
    }
    let left_rows = merge_all(sources.left, work)?;
    let right_rows = merge_all(sources.right, work)?;
    Ok(Box::new(left_rows.chain(right_rows).map(|r| r.map(|(_, row)| row))))
}

/// The right operand's numbered rows of a null key, `records`, each after
/// nulls if the right is preserved, in a run of `work`; the right's own
/// condition, `judged` if the left has a row, judged of each, though such
/// a row matches nothing.
fn alone_rights(records: Records, (spec, judged): (&Spec, Option<&Cond>), work: &mut Work) -> Result<Records> {
    let mut out = work.run()?;
    for record in records {
        let (n, row) = unnumbered(&record?, work.types())?;
        if judged.is_some() {
            spec.right_may_match(&row)?;
        }
        if spec.kind.keeps_right() {
            out.push(&numbered(n, &spec.right_alone(&row)))?;
        }
    }
    out.into_records()
}

/// Joins the right partition `right`, of `bytes`, with its left partition
/// `probes`, at `depth` from the first, their rows added to `sources`, the
/// right's own condition `judged` of each of its rows, if given: the pair
/// partitioned again if the right outgrows the work area and the depth
/// allows, else joined a part of the right at a time.
fn pair(
    (right, bytes, probes): (Box<dyn SpillRun>, usize, Box<dyn SpillRun>),
    (spec, judged, depth): (&Spec, Option<&Cond>, usize),
    sources: &mut Sources,
    work: &mut Work,
) -> Result<()> {
    if bytes <= work.limit() || depth >= work.depth() {
        return chunked::joined((right.into_records()?, probes), (spec, judged), sources, work);
    }
    let parts = Parts::new(spec, work)?;
    let rights = parts.split(right.into_records()?, (spec, true), work)?;
    // A partition that did not shrink is joined in parts, not partitioned on.
    let shrank = rights.iter().all(|(_, b)| *b < bytes);
    let lefts = parts.split(probes.into_records()?, (spec, false), work)?;
    for ((sub, sub_bytes), sub_probes) in rights.into_iter().zip(lefts.into_iter().map(|(run, _)| run)) {
        pair((sub, sub_bytes, sub_probes), (spec, judged, if shrank { depth.saturating_add(1) } else { usize::MAX }), sources, work)?;
    }
    Ok(())
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

    /// `count` new runs of `work`, each with no bytes yet.
    fn runs(&self, work: &mut Work) -> Result<Vec<(Box<dyn SpillRun>, usize)>> {
        (0..self.count).map(|_| work.run().map(|run| (run, 0))).collect()
    }

    /// The right operand `rows`, numbered, in partitions, each with the
    /// bytes its rows take held in memory; and, of a preserved right or
    /// one with a condition of its own, its rows with a null key.
    fn rights(&self, rows: impl Iterator<Item = Result<Row>>, spec: &Spec, work: &mut Work) -> Result<Partitioned> {
        let mut runs = self.runs(work)?;
        let mut lonely = work.run()?;
        for (n, r) in (0_u64..).zip(rows) {
            let row = r?;
            match key(&row, spec.keys.iter().map(|k| k.1))? {
                Some(b) => {
                    let (run, bytes) = runs.get_mut(self.of(&b)).ok_or_else(lost)?;
                    *bytes = bytes.saturating_add(row_bytes(&row)).saturating_add(PLACE_BYTES);
                    run.push(&numbered(n, &row))?;
                }
                None => {
                    if spec.kind.keeps_right() || spec.own.1.is_some() {
                        lonely.push(&numbered(n, &row))?;
                    }
                }
            }
        }
        Ok((runs, lonely))
    }

    /// The left operand `rows`, numbered, in partitions; of a preserved
    /// left, its rows with a null key or not matched as their own condition
    /// is not true of them, with nulls; and whether it had a row. The right
    /// has rows, as it outgrew the work area, so the left's own condition
    /// is judged of each [ISO/IEC 9075:1992 §7.5 GR1c].
    fn lefts(&self, rows: RowIter, spec: &Spec, work: &mut Work) -> Result<Lefts> {
        let mut runs = self.runs(work)?;
        let mut lonely = work.run()?;
        let mut any = false;
        for (n, r) in (0_u64..).zip(rows) {
            let row = r?;
            any = true;
            let keyed = if spec.left_may_match(&row)? { key(&row, spec.keys.iter().map(|k| k.0))? } else { None };
            match keyed {
                Some(b) => runs.get_mut(self.of(&b)).ok_or_else(lost)?.0.push(&numbered(n, &row))?,
                None => {
                    if spec.kind.keeps_left() {
                        lonely.push(&numbered(n, &spec.left_alone(&row)))?;
                    }
                }
            }
        }
        Ok((runs.into_iter().map(|(run, _)| run).collect(), lonely, any))
    }

    /// The numbered rows of a partition, `records`, of the right operand
    /// if `right`, else of the left, in partitions again, each with the
    /// bytes its rows take held in memory.
    fn split(&self, records: Records, (spec, right): (&Spec, bool), work: &mut Work) -> Result<Vec<(Box<dyn SpillRun>, usize)>> {
        let mut runs = self.runs(work)?;
        for record in records {
            let record = record?;
            let (_, row) = unnumbered(&record, work.types())?;
            let columns = spec.keys.iter().map(|&(l, r)| if right { r } else { l });
            let b = key(&row, columns)?.ok_or_else(lost)?;
            let (run, bytes) = runs.get_mut(self.of(&b)).ok_or_else(lost)?;
            *bytes = bytes.saturating_add(row_bytes(&row)).saturating_add(PLACE_BYTES);
            run.push(&record)?;
        }
        Ok(runs)
    }
}

/// The right operand's partitions, each with its bytes, and its rows of a
/// null key.
type Partitioned = (Vec<(Box<dyn SpillRun>, usize)>, Box<dyn SpillRun>);

/// The left operand's partitions, its rows that match nothing, and
/// whether it had a row.
type Lefts = (Vec<Box<dyn SpillRun>>, Box<dyn SpillRun>, bool);

/// The error of a partition not as it was written.
pub(super) fn lost() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] an outer join's partition is not as it was written")
}
