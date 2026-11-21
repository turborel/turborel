//! \[DESIGN\] (RULES.md D667) Which steps of a chain's tree a phase takes,
//! and whether it is planned. A step's filter holds the keys of the leaf
//! it is made of; of a leaf of `rows` rows whose key has `d` values, those
//! are at most min(rows, d). It is kept only where they are at most
//! `TRANSFER_SHARE` of the values the column it judges has, so it is
//! expected to drop at least half of that leaf's rows, as RPT+ (Qiao,
//! Boncz and Zhang, PVLDB 2026) builds a filter only where it is expected
//! to pay; each leaf's rows are those its own conditions keep, times the
//! share each step before kept of them, its keys' values taken as
//! independent and spread alike (Selinger et al., 1979).
//!
//! The phase is planned if the rows its steps are expected to drop, each
//! worth `DROPPED_ROW`, outweigh a Bloom filter's insert and probe of
//! every row of each step, `BLOOM_ROW` each, and the phase's own
//! `PHASE_ROWS`. Of the chain's first leaf, a step's rows dropped are
//! counted only past those a filter of its leaf as planned would drop:
//! its scan is told the filters of the builds joined with it already
//! (RULES.md D369).

use super::Step;
use super::chain::{Key, keys, leaves};
use super::tree::schedule;
use crate::db::State;
use crate::physical::Plan;

/// \[DESIGN\] The most the keys of a step's filter may be of the values of
/// the column it judges: one in two, RPT+'s.
const TRANSFER_SHARE: f64 = 0.5;

/// \[DESIGN\] The cost of a row inserted into or probing a Bloom filter,
/// its key hashed, in the cost model's units of a row read: a tenth.
const BLOOM_ROW: f64 = 0.1;

/// \[DESIGN\] What a row dropped before the joins saves: its insert into a
/// join's table or its probe of one, and the rows it would make of the
/// joins above, half a row read.
const DROPPED_ROW: f64 = 0.5;

/// \[DESIGN\] The phase's own cost, in rows read: of holding its leaves
/// and making its filters, so a chain of few rows keeps its plan.
const PHASE_ROWS: f64 = 200.0;

/// \[DESIGN\] The most leaves a chain's phase is planned of: the tree's
/// search takes a pass over the keys for each leaf.
const MOST_LEAVES: usize = 64;

/// A leaf, its rows as planned, and as the steps before reduce them.
struct Reduced<'p> {
    planned: f64,
    rows: f64,
    leaf: &'p Plan,
}

/// The steps of a phase over the chain `plan`, and what EXPLAIN shows of
/// it, if one is worth planning.
pub(super) fn planned(db: &State, plan: &Plan) -> Option<(Vec<Step>, String)> {
    if plan.can_raise() || !crate::exec::transferable(plan, db) {
        return None;
    }
    let found = leaves(plan);
    if found.len() < 2 || found.len() > MOST_LEAVES {
        return None;
    }
    let rows: Vec<f64> = found.iter().map(|l| l.est().unwrap_or(1.0_f64).max(1.0_f64)).collect();
    let keys = keys(plan, db);
    let edges: Vec<Step> = keys.iter().map(|k| k.step).collect();
    let (up, down) = schedule(&rows, &edges)?;
    let mut reduced: Vec<Reduced<'_>> = found.iter().zip(&rows).map(|(&leaf, &rows)| Reduced { planned: rows, rows, leaf }).collect();
    let (mut steps, mut gain) = (Vec::new(), -PHASE_ROWS);
    for step in up.into_iter().chain(down).filter(|s| allowed(&keys, *s)) {
        if let Some(dropped) = taken(db, &mut reduced, step) {
            steps.push(step);
            gain += dropped;
        }
    }
    let detail = format!("of {} leaves, {} steps", found.len(), steps.len());
    worth(gain, steps.len()).then_some((steps, detail))
}

/// Of `step`, if it is kept: what it is expected to gain, the rows it
/// drops less what its filter costs, its leaf's rows reduced by it.
fn taken(db: &State, reduced: &mut [Reduced<'_>], step: Step) -> Option<f64> {
    let from = reduced.get(step.from.0)?;
    let of = distinct(db, from.leaf, step.from.1);
    let (keys, planned) = (from.rows.min(of.unwrap_or(from.rows)), from.planned.min(of.unwrap_or(from.planned)));
    let to = reduced.get_mut(step.to.0)?;
    let values = distinct(db, to.leaf, step.to.1).unwrap_or(to.rows).max(1.0_f64);
    if !keeps(keys, values) {
        return None;
    }
    let before = to.rows;
    to.rows = (before * keys / values).max(1.0_f64);
    // The first leaf's scan keeps what a runtime filter of the leaf as
    // planned would have of it anyway (RULES.md D369).
    let kept = if step.to.0 == 0 { (before * planned / values).min(before) } else { before };
    Some((kept - to.rows).max(0.0_f64) * DROPPED_ROW - (keys + before) * BLOOM_ROW)
}

/// The values of column `column` of the rows of `leaf`, if it is a scan
/// of a table, filtered or not, whose statistics tell.
fn distinct(db: &State, leaf: &Plan, column: usize) -> Option<f64> {
    let mut at = leaf.bare();
    for _ in 0..super::chain::LINKS {
        if let Plan::SeqScan { table, .. } = at {
            return db.statistics(table)?.distinct(column);
        }
        at = if let Plan::Parallel(p) = at {
            p.input.bare()
        } else if let Plan::Filter { input, .. } = at {
            input.bare()
        } else {
            return None;
        };
    }
    None
}

/// Whether `step` goes the way its key transfers: either way, or of a LEFT
/// join's key, from its left operand to its right (`chain.rs`).
fn allowed(keys: &[Key], step: Step) -> bool {
    keys.iter().any(|k| k.step == step || (k.both && k.step == Step { from: step.to, to: step.from, nulls: step.nulls }))
}

/// Whether a step whose filter holds `keys` keys judging a column of
/// `values` values is kept: of at most `TRANSFER_SHARE` of them.
fn keeps(keys: f64, values: f64) -> bool {
    keys <= TRANSFER_SHARE * values
}

/// Whether a phase of `steps` steps, expected to gain `gain`, is planned:
/// of a step at least, and a gain.
fn worth(gain: f64, steps: usize) -> bool {
    gain > 0.0_f64 && steps > 0
}

// Unit tests: the step kept, the phase planned (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D667: a step is kept of keys at most
    /// half the values it judges, so it is expected to drop half its rows.
    #[test]
    fn a_step_keeps_half_the_values_at_most() {
        assert!(keeps(50.0_f64, 100.0_f64));
        assert!(!keeps(51.0_f64, 100.0_f64));
    }

    /// Verifies: [DESIGN] RULES.md D665 (MC/DC): a step goes the way of its
    /// key, either way of an inner join's, of a LEFT join's from its left
    /// to its right alone.
    #[test]
    fn a_left_joins_key_transfers_to_its_right_alone() {
        let step = Step { from: (0, 1), to: (1, 2), nulls: false };
        let back = Step { from: (1, 2), to: (0, 1), nulls: false };
        assert!(allowed(&[Key { step, both: true }], back), "an inner join's, back");
        assert!(allowed(&[Key { step, both: false }], step), "a LEFT join's, to its right");
        assert!(!allowed(&[Key { step, both: false }], back), "a LEFT join's, back");
    }

    /// Verifies: [DESIGN] RULES.md D667 (MC/DC): a phase is planned of a
    /// gain and a step; no gain, or no step, none.
    #[test]
    fn a_phase_is_planned_of_a_gain_and_a_step() {
        assert!(worth(1.0_f64, 1_usize));
        assert!(!worth(0.0_f64, 1_usize), "no gain");
        assert!(!worth(1.0_f64, 0_usize), "no step");
    }
}
