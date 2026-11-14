//! [ISO/IEC 9075-2:2023 §7.7 GR11b ii 2 B II 2, §4.23.2] The measures of
//! ALL ROWS PER MATCH, row after row of one match. The rows mapped to each
//! variable are found once for the match; a running value reads those up
//! to the current row, a final one all of them, computed once for the
//! match. A running set function is fed each row once as the current row
//! moves forward, and finished at each, so a match's rows cost each value
//! a number of steps linear in their count.

use super::aggregate::Fed;
use super::scene::{Reading, Scene, reach};
use crate::error::Result;
use crate::exec::{broken, eval};
use crate::logical::{AggCall, Expr, PatternValue, Slotted};
use crate::pattern::Mapped;
use crate::storage::Row;
use crate::value::Value;

/// What is kept of a value between rows of a match.
enum Kept {
    /// Nothing yet, or nothing to keep.
    Nothing,
    /// A final value's.
    Final(Value),
    /// A running set function fed so many of its variable's rows.
    Running(Box<Fed>, usize),
}

/// The measures' values of a match, row after row.
pub(super) struct Measures {
    /// Of each variable, the places from the match's first row of the rows
    /// mapped to it.
    members: Vec<Vec<usize>>,
    /// Of each of the measures' values, what is kept of it.
    kept: Vec<Kept>,
}

impl Measures {
    /// Those of the match `mapped` of `reading`, of `values` values.
    pub(super) fn new(reading: &Reading<'_>, mapped: &[Mapped], values: usize) -> Measures {
        let count = reading.matching.variables.len();
        let members = (0..count).map(|v| reading.members(mapped, v, mapped.len())).collect();
        Measures { members, kept: (0..values).map(|_| Kept::Nothing).collect() }
    }

    /// The values of `measures` of `scene`, its current row the `upto`th of
    /// the match, `upto` never less than the row before's.
    ///
    /// # Errors
    ///
    /// Their values' and expressions' evaluations'.
    pub(super) fn row(&mut self, reading: &Reading<'_>, scene: &mut Scene<'_>, measures: &Slotted<Vec<Expr>>, upto: usize) -> Result<Row> {
        let mut slots = Vec::with_capacity(measures.values.len());
        for (v, kept) in measures.values.iter().zip(self.kept.iter_mut()) {
            slots.push(value((&self.members, kept), reading, scene, v, upto)?);
        }
        measures.expr.iter().map(|e| eval(e, &slots, reading.runner)).collect()
    }
}

/// The value `v` reads of `scene`, of the rows of each variable `members`,
/// what is kept of it `kept`, its current row the `upto`th of the match.
fn value(
    (members, kept): (&[Vec<usize>], &mut Kept),
    reading: &Reading<'_>,
    scene: &mut Scene<'_>,
    v: &PatternValue,
    upto: usize,
) -> Result<Value> {
    if let Kept::Final(value) = kept {
        return Ok(value.clone());
    }
    let (variable, final_) = match v {
        PatternValue::Navigated(n) => (n.variable, n.final_),
        PatternValue::Aggregate(a) => (a.variable, a.final_),
        PatternValue::MatchNumber => return Reading::number(scene),
    };
    let all = members.get(variable).ok_or_else(|| broken("a row pattern value of no variable"))?;
    let reached = reach(final_, upto, scene.mapped);
    let set = all.get(..all.partition_point(|&j| j < reached)).unwrap_or_default();
    let found = match v {
        PatternValue::Navigated(n) => reading.navigated(scene, n, set)?,
        PatternValue::Aggregate(a) => {
            if final_ {
                reading.aggregated(scene, a, set)?
            } else {
                running(kept, reading, scene, &a.call, set)?
            }
        }
        PatternValue::MatchNumber => return Reading::number(scene),
    };
    if final_ {
        *kept = Kept::Final(found.clone());
    }
    Ok(found)
}

/// The running set function `call` of the rows `set` of its variable, up
/// to the current row, fed those of them it was not fed yet, of what is
/// kept of it `kept`. The current row never moves back within a match
/// (`Measures::row`), so the rows it was fed are always the first of
/// `set`, and a set function once fed is fed on, never begun afresh.
fn running(kept: &mut Kept, reading: &Reading<'_>, scene: &mut Scene<'_>, call: &AggCall, set: &[usize]) -> Result<Value> {
    if let Kept::Nothing | Kept::Final(_) = kept {
        *kept = Kept::Running(Box::new(Fed::new(call, reading)), 0);
    }
    let Kept::Running(acc, fed) = kept else { return Err(broken("a running set function kept nothing")) };
    for &j in set.get(*fed..).unwrap_or_default() {
        acc.feed(call, reading, scene, scene.start.saturating_add(j))?;
    }
    *fed = set.len();
    acc.finish(call, reading, scene)
}
