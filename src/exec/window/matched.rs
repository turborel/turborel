//! [ISO/IEC 9075-2:2023 §7.15 GR6, §6.10 GR1e] A window's row pattern
//! recognition (R020). Of each row of a partition, in order, its full
//! frame is found as any frame is, its first row the row itself [SR14c];
//! unless its skip indicator is set, its designated match is sought in the
//! frame: from its first row, or of SEEK from each of its rows in turn
//! until one is found [GR6d iii 2 B]. The frame is then narrowed to the
//! match's rows, emptied of no match or a skip indicator set [GR6d iii 2
//! D], so every function over it reads only them, and the match's
//! measures, evaluated in its last row, are the values of its `<window row
//! pattern measure>`s. The skip indicators a match sets are the rows after
//! the current one up to a place [GR6d iii 2 E], so the greatest such
//! place stands for them all. Calls of one window share one matching,
//! sought once for each row.

use std::sync::Arc;

use super::Count;
use super::frame::{Bounds, Current, EMPTY, Partition, Ranges};
use crate::error::Result;
use crate::exec::pattern::scene::{Reading, Scene};
use crate::exec::work::Work;
use crate::exec::{HeldRows, Subqueries};
use crate::logical::{Skip, WindowCall, WindowKind, WindowMatching};
use crate::pattern::{Mapped, Program};
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::value::Value;

/// [GR6d iii 2 E III] The rules SKIP TO breaks in a window: of no row
/// mapped to its variable, and of its row the match's first.
const SKIP_RULES: (&str, &str) = ("[ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 E III 1]", "[ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 E III 2, 3]");

/// What a window's row pattern recognition reads with: the runner of its
/// subqueries, the work area and the space a set function's values are
/// kept in.
pub(super) type Readers<'r> = (&'r dyn Subqueries, &'r Work, &'r ListSpace);

/// The row pattern recognition of the calls of a window, row after row of
/// a partition.
pub(super) struct Matched<'a> {
    groups: Vec<Group<'a>>,
    /// Of each call, the group of its frame's matching, if it has one.
    of: Vec<Option<usize>>,
}

/// One matching, shared by the calls whose frames have it.
struct Group<'a> {
    matching: &'a Arc<WindowMatching>,
    program: Program,
    /// The call whose bounds give each row's full frame.
    call: usize,
    /// Whether a call reads its measures.
    measured: bool,
    /// [GR6d ii] The rows after the last current row and before this place
    /// have their skip indicators set.
    skipped: usize,
    /// The current row's designated match, if any: the place of its first
    /// row, its count of rows, and its measures' values.
    designated: Option<(usize, usize, Row)>,
}

impl<'a> Matched<'a> {
    /// That of `calls`.
    ///
    /// # Errors
    ///
    /// \[DESIGN\] A pattern beyond the matcher's limits.
    pub(super) fn new(calls: &'a [WindowCall]) -> Result<Matched<'a>> {
        let (mut groups, mut of) = (Vec::<Group<'a>>::new(), Vec::with_capacity(calls.len()));
        for (n, call) in calls.iter().enumerate() {
            let Some(matching) = &call.frame.matching else {
                of.push(None);
                continue;
            };
            let measured = matches!(call.kind, WindowKind::Measure(_));
            if let Some((at, g)) = groups.iter_mut().enumerate().find(|(_, g)| Arc::ptr_eq(g.matching, matching)) {
                if measured {
                    g.measured = true;
                }
                of.push(Some(at));
            } else {
                let program = Program::new(&matching.matching.pattern)?;
                of.push(Some(groups.len()));
                groups.push(Group { matching, program, call: n, measured, skipped: 0, designated: None });
            }
        }
        Ok(Matched { groups, of })
    }

    /// Begins a partition, with no skip indicator set.
    pub(super) fn reset(&mut self) {
        for g in &mut self.groups {
            (g.skipped, g.designated) = (0, None);
        }
    }

    /// [GR6d iii] Finds the designated match of `current` of each matching,
    /// its full frame of `bounds` of `part`, of the rows `rows`, read with
    /// `readers`.
    ///
    /// # Errors
    ///
    /// The definitions' and measures' evaluations'; the search's;
    /// SQLSTATE 2202K or 2202L of SKIP TO [GR6d iii 2 E III].
    pub(super) fn designate(
        &mut self,
        current: &Current,
        (bounds, part): (&mut [(Bounds, Count)], &mut Partition),
        rows: &mut HeldRows<Row>,
        (runner, work, space): Readers<'_>,
    ) -> Result<()> {
        for g in &mut self.groups {
            g.designated = None;
            // [GR6d iii 1] a row whose skip indicator is set has no frame.
            if current.row < g.skipped {
                continue;
            }
            let Some((b, _)) = bounds.get_mut(g.call) else { continue };
            let [(from, to), ..] = b.frame(current, part)?;
            let reading = Reading { matching: &g.matching.matching, runner, work, space };
            g.designated = g.seek(&reading, rows, (from, to), current.row)?;
        }
        Ok(())
    }

    /// The frame of the call numbered `call`, narrowed to its row's
    /// designated match [GR6d iii 2 D]; none if it has no matching.
    pub(super) fn frame(&self, call: usize) -> Option<Ranges> {
        let g = self.of.get(call).copied().flatten().and_then(|at| self.groups.get(at))?;
        let narrowed = g.designated.as_ref().filter(|(_, len, _)| *len > 0);
        Some(narrowed.map_or([EMPTY, EMPTY, EMPTY], |&(start, len, _)| [(start, start.saturating_add(len)), EMPTY, EMPTY]))
    }

    /// [ISO/IEC 9075-2:2023 §6.10 GR1e] The `<window row pattern measure>`
    /// numbered `k` of the call numbered `call`: the measure evaluated in
    /// its row's designated match, null if it has none, its skip indicator
    /// set.
    pub(super) fn measure(&self, call: usize, k: usize) -> Value {
        let g = self.of.get(call).copied().flatten().and_then(|at| self.groups.get(at));
        g.and_then(|g| g.designated.as_ref()).and_then(|(_, _, measures)| measures.get(k).cloned()).unwrap_or(Value::Null)
    }
}

impl Group<'_> {
    /// [GR6d iii 2 B to E] The designated match of the row `row` in its
    /// full frame `from` to `to`, of `rows`, read with `reading`, its skip
    /// indicators set.
    fn seek(
        &mut self,
        reading: &Reading<'_>,
        rows: &mut HeldRows<Row>,
        (from, to): (usize, usize),
        row: usize,
    ) -> Result<Option<(usize, usize, Row)>> {
        // A frame of no rows, `to` not past `from`, has no match, as no
        // row of it is tried, of SEEK or not.
        let tried = if self.matching.matching.seek { from..to } else { from..to.min(from.saturating_add(1)) };
        for start in tried {
            let found = self.program.find((from, start, to), &mut |variable, path| {
                let define = reading.matching.defines.get(variable).and_then(Option::as_ref);
                let mut scene = Scene { rows: &mut *rows, start, mapped: path, number: None };
                reading.holds(&mut scene, define)
            })?;
            let Some(mapped) = found else { continue };
            let len = mapped.len();
            let measures = if self.measured {
                let mut scene = Scene { rows: &mut *rows, start, mapped: &mapped, number: None };
                reading.measured(&mut scene, &self.matching.measures, len)?
            } else {
                Vec::new()
            };
            self.skipped = self.skipped.max(skipped_to(reading, start, &mapped)?.unwrap_or(row));
            return Ok(Some((start, len, measures)));
        }
        Ok(None)
    }
}

/// [GR6d iii 2 E] The place before which the rows after the current one
/// have their skip indicators set by the match from `start`, `mapped`;
/// none of an empty match or of SKIP TO NEXT ROW.
///
/// # Errors
///
/// SQLSTATE 2202K or 2202L of SKIP TO [GR6d iii 2 E III].
fn skipped_to(reading: &Reading<'_>, start: usize, mapped: &[Mapped]) -> Result<Option<usize>> {
    if mapped.is_empty() {
        return Ok(None);
    }
    let j = match reading.matching.skip {
        Skip::NextRow => return Ok(None),
        Skip::PastLastRow => mapped.len(),
        Skip::First(v) => reading.skipped_to(mapped, (v, false), SKIP_RULES)?,
        Skip::Last(v) => reading.skipped_to(mapped, (v, true), SKIP_RULES)?,
    };
    Ok(Some(start.saturating_add(j)))
}

// Unit tests: frames narrowed to matches, skip indicators, SEEK
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
