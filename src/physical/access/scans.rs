//! \[DESIGN\] The index scans of an access path chosen (RULES.md D34):
//! one for each probe of an index, appended in key order.

use super::Probed;
use crate::logical::{Cond, and_all, render_cond};
use crate::physical::probe::render_probe;
use crate::physical::{Plan, TableRead};
use crate::storage::cost_of;

/// The index scans of `probed` on the table `read`, answering the
/// conjuncts `answered`, of the rows `est` and the `cost` the planner gave
/// them all, read `reverse` if so: one for each of its probes, the rows of
/// each after those of the one before (an OR's ranges, none overlapping,
/// so each row is found once, and in key order, so the rows are in the
/// index's order, or, read backward, the last range's first). Each shows
/// the conjuncts it answers, or its probe, if it answers some only in
/// part, or one of several.
pub(super) fn index_scans(read: TableRead<'_>, probed: Probed, answered: Vec<Cond>, (est, cost, reverse): (f64, f64, bool)) -> Plan {
    let share = cost_of(probed.probes.len());
    let whole =
        and_all(answered).filter(|_| !probed.loose && probed.probes.len() == 1).map(|c| read.shown.text(|names| render_cond(&c, names)));
    let Probed { index, method, columns, mut probes, mut slots, covering, .. } = probed;
    if reverse {
        probes.reverse();
    }
    let mut level: Vec<Plan> = probes
        .into_iter()
        .map(|probe| {
            let detail = whole.clone().unwrap_or_else(|| read.shown.text(|names| render_probe(&probe, &columns, names)));
            let (index, method, table) = (index.clone(), method.clone(), read.table.clone());
            let needed = read.needed.to_vec();
            // The `?`s are of a probe alone, the one of a point path.
            let slots = std::mem::take(&mut slots);
            Plan::IndexScan {
                table,
                index,
                method,
                probe,
                slots,
                reverse,
                columns: needed,
                detail,
                est: est / share,
                cost: cost / share,
                covering,
                located: read.located,
            }
        })
        .collect();
    // Appended in pairs, a level at a time, so they are about log2(n) deep
    // (RULES.md R-CF-6).
    while level.len() > 1 {
        let mut pairs = level.into_iter();
        let mut next = Vec::new();
        while let Some(left) = pairs.next() {
            next.push(match pairs.next() {
                Some(right) => Plan::Append { left: Box::new(left), right: Box::new(right) },
                None => left,
            });
        }
        level = next;
    }
    level.pop().unwrap_or(Plan::Values { rows: Vec::new(), detail: String::new() })
}
