//! \[DESIGN\] A pair of an outer join's partitions joined a part of the
//! right partition at a time (RULES.md D38): as many of its rows as the
//! work area holds, with their places in their buckets and their flags,
//! whether they matched, hashed in memory; the left partition read again
//! for each part, each left row joined with its matches there. The right
//! rows of a part that matched none are given once it is done. Whether a
//! left row matched in a part before is a flag of it in a run, read beside
//! the left rows and written again for the next part; at the last part, a
//! preserved left row that matched in none is given with nulls. A partition
//! that fits the work area is one part, so no flag of a left row is
//! written. Nothing held grows past the work area, however many rows have
//! one key. Split from exec/join/outer/spilled.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::iter::Peekable;

use super::super::super::partition::{numbered, unnumbered};
use super::super::super::work::Work;
use super::super::key;
use super::spilled::{Sources, lost};
use super::{PLACE_BYTES, Spec};
use crate::error::Result;
use crate::logical::Cond;
use crate::spill::{Records, SpillRun, row_bytes};
use crate::storage::Row;
use crate::value::Bucket;

/// A left row's flag, in a run: it matched a right row of a part before.
const MATCHED: u8 = 1;
/// A left row's flag, in a run: it matched none yet.
const UNMATCHED: u8 = 0;

/// Joins the numbered rows of a right partition, `right`, with those of
/// its left partition, `probes`, a part of `right` at a time, as `spec`
/// says, the right's own condition `judged` of each of its rows, if
/// given, their rows added to `sources`.
pub(super) fn joined(
    (right, mut probes): (Records, Box<dyn SpillRun>),
    (spec, judged): (&Spec, Option<&Cond>),
    sources: &mut Sources,
    work: &mut Work,
) -> Result<()> {
    let mut right = right.peekable();
    let mut before: Option<Records> = None;
    loop {
        let mut part = Part::of(&mut right, (spec, judged.is_some()), work)?;
        let last = right.peek().is_none();
        let mut out = work.run()?;
        let mut flags = if last { None } else { Some(work.run()?) };
        for record in probes.records()? {
            let (n, l) = unnumbered(&record?, work.types())?;
            let was = match before.as_mut().and_then(Iterator::next).transpose()? {
                Some(flag) => flag.first() == Some(&MATCHED),
                None => false,
            };
            let found = part.probed((n, &l), spec, &mut *out)? || was;
            match &mut flags {
                Some(flags) => flags.push(&[if found { MATCHED } else { UNMATCHED }])?,
                None => {
                    if !found && spec.kind.keeps_left() {
                        out.push(&numbered(n, &spec.left_alone(&l)))?;
                    }
                }
            }
        }
        sources.left.push(out.into_records()?);
        if spec.kind.keeps_right() {
            sources.right.push(part.unmatched(spec, work)?);
        }
        let Some(flags) = flags else { return Ok(()) };
        before = Some(flags.into_records()?);
    }
}

/// A part of a right partition, hashed: its numbered rows, the places of
/// each bucket's, and each row's flag, whether it matched.
struct Part {
    rows: Vec<(u64, Row)>,
    buckets: HashMap<Vec<Bucket>, Vec<usize>>,
    matched: Vec<bool>,
}

impl Part {
    /// The next rows of `right`, numbered rows hashed on their columns of
    /// `spec`'s keys, as many as `work` holds, and one at least; if
    /// `judged`, those the right's own condition is not true of held
    /// unhashed, so they match none.
    fn of(right: &mut Peekable<Records>, (spec, judged): (&Spec, bool), work: &Work) -> Result<Part> {
        let mut part = Part { rows: Vec::new(), buckets: HashMap::new(), matched: Vec::new() };
        let mut bytes = 0_usize;
        while bytes <= work.limit() {
            let Some(record) = right.next() else { break };
            let (n, row) = unnumbered(&record?, work.types())?;
            bytes = bytes.saturating_add(row_bytes(&row)).saturating_add(PLACE_BYTES);
            let b = key(&row, spec.keys.iter().map(|k| k.1))?.ok_or_else(lost)?;
            if !judged || spec.right_may_match(&row)? {
                part.buckets.entry(b).or_default().push(part.rows.len());
            }
            part.rows.push((n, row));
            part.matched.push(false);
        }
        Ok(part)
    }

    /// Left row `l`, numbered `n`, joined with each of its rows that
    /// matches it, those numbered in `out`, the rows marked; whether one
    /// did.
    fn probed(&mut self, (n, l): (u64, &Row), spec: &Spec, out: &mut dyn SpillRun) -> Result<bool> {
        let Some(b) = key(l, spec.keys.iter().map(|k| k.0))? else { return Ok(false) };
        let mut found = false;
        for &at in self.buckets.get(&b).map(Vec::as_slice).unwrap_or_default() {
            let (_, r) = self.rows.get(at).ok_or_else(lost)?;
            let Some(joined) = spec.joined(l, r)? else { continue };
            found = true;
            if let Some(m) = self.matched.get_mut(at) {
                *m = true;
            }
            out.push(&numbered(n, &joined))?;
        }
        Ok(found)
    }

    /// Its rows that matched none, after nulls, each with its number, in a
    /// run of `work`, read from the first.
    fn unmatched(self, spec: &Spec, work: &mut Work) -> Result<Records> {
        let mut out = work.run()?;
        for ((n, row), matched) in self.rows.into_iter().zip(self.matched) {
            if !matched {
                out.push(&numbered(n, &spec.right_alone(&row)))?;
            }
        }
        out.into_records()
    }
}
