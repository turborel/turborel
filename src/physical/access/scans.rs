//! \[DESIGN\] The index scans of an access path chosen (RULES.md D34):
//! one for each probe of an index, appended in key order.

use super::Probed;
use crate::logical::{Cond, and_all, render_cond};
use crate::physical::probe::render_probe;
use crate::physical::{Decided, Plan, TableRead};
use crate::storage::cost_of;

/// The index scans of `probed` on the table `read`, answering the
/// conjuncts `answered`, of the rows `est` and the `cost` the planner gave
/// them all, read `reverse` if so: one for each of its probes, the rows of
/// each after those of the one before (an OR's ranges, none overlapping,
/// so each row is found once, and in key order, so the rows are in the
/// index's order, or, read backward, the last range's first). Each shows
/// the conjuncts it answers, or its probe, if it answers some only in
/// part, or one of several, and that it decides a conjunct of each entry,
/// if it does (RULES.md D85): the conjunct is each scan's, and the column
/// it alone reads is read of no row.
pub(super) fn index_scans(read: TableRead<'_>, probed: Probed, answered: Vec<Cond>, (est, cost, reverse): (f64, f64, bool)) -> Plan {
    let share = cost_of(probed.probes.len());
    let whole =
        and_all(answered).filter(|_| !probed.loose && probed.probes.len() == 1).map(|c| read.shown.text(|names| render_cond(&c, names)));
    let Probed { index, method, columns, mut probes, mut slots, covering, decided, .. } = probed;
    if reverse {
        probes.reverse();
    }
    // The column the decided conjunct alone reads is read of no row.
    let unread = decided.as_ref().filter(|d| d.unread).map(|d| d.column);
    let needed: Vec<usize> = read.needed.iter().copied().filter(|&c| Some(c) != unread).collect();
    let decided = decided.and_then(|d| Some(Box::new(Decided { cond: d.cond?, column: d.column })));
    let deciding = if decided.is_some() && read.shown.names().is_some() { ", decided of each entry" } else { "" };
    let mut level: Vec<Plan> = probes
        .into_iter()
        .map(|probe| {
            let shown = whole.clone().unwrap_or_else(|| read.shown.text(|names| render_probe(&probe, &columns, names)));
            let detail = format!("{shown}{deciding}");
            let (index, method, table) = (index.clone(), method.clone(), read.table.clone());
            // The `?`s are of a probe alone, the one of a point path.
            let slots = std::mem::take(&mut slots);
            Plan::IndexScan {
                table,
                index,
                method,
                probe,
                slots,
                reverse,
                columns: needed.clone(),
                detail,
                est: est / share,
                cost: cost / share,
                covering,
                located: read.located,
                decided: decided.clone(),
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
                Some(right) => {
                    let appended = Plan::summed(&left, &right);
                    Plan::Append { left: Box::new(left), right: Box::new(right), est: appended }
                }
                None => left,
            });
        }
        level = next;
    }
    level.pop().unwrap_or(Plan::Values { rows: Vec::new(), detail: String::new() })
}
