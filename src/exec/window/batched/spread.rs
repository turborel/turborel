//! \[DESIGN\] RULES.md D560: a window's whole partitions computed on
//! several threads. The rows held, every partition of them whole, are cut
//! at partition starts into a group for each thread, near equal in rows;
//! each group's calls are computed by a morsel of the statement's run
//! (`exec/parallel/run.rs`), the groups held in place, so a group that
//! raises an error leaves every row where it was for the window to give
//! way with. The groups' values come back in order, so the rows keep it.

use std::sync::{Arc, Mutex, PoisonError};

use super::Held;
use super::compute::{Prepared, computed};
use super::lanes::Built;
use super::reading::split;
use crate::error::Result;
use crate::exec::parallel::Morsels;
use crate::exec::{Db, Runner};
use crate::logical::Window;
use crate::txn::TxId;

/// The groups computed, each its rows held and, unless one failed, the
/// calls' values of its rows.
pub(super) type Computed = (Vec<Vec<Held>>, Result<Vec<Vec<Built>>>);

/// `held`, rows of whole partitions, cut at partition starts into at most
/// `degree` groups of near `rows / degree` rows each, in order.
pub(super) fn groups(held: Vec<Held>, degree: usize) -> Vec<Vec<Held>> {
    let rows = held.iter().map(|h| h.batch.rows).fold(0, usize::saturating_add);
    let target = rows.checked_div(degree.max(1)).unwrap_or(rows).max(1);
    let (mut out, mut group, mut taken) = (Vec::new(), Vec::new(), 0_usize);
    for mut next in held {
        let begins = next.parts.first().copied().unwrap_or(false);
        if begins && taken >= target && !group.is_empty() && out.len().saturating_add(1) < degree {
            out.push(std::mem::take(&mut group));
            taken = 0;
        }
        loop {
            let need = target.saturating_sub(taken);
            let at = next.parts.iter().enumerate().skip(need.max(1)).find(|(_, p)| **p).map(|(at, _)| at);
            let Some(cut) = at.filter(|_| out.len().saturating_add(1) < degree) else { break };
            let (before, after) = split(next, cut);
            group.extend(before);
            out.push(std::mem::take(&mut group));
            (next, taken) = (after, 0);
        }
        taken = taken.saturating_add(next.batch.rows);
        group.push(next);
    }
    if !group.is_empty() {
        out.push(group);
    }
    out
}

/// The calls of `w` computed of each of `groups`, each group by a morsel
/// on up to `degree` threads of `db`'s, its trees of at most `bytes` each.
pub(super) fn computed_apart(
    groups: Vec<Vec<Held>>,
    (w, prepared): (Arc<Window<()>>, Arc<Prepared>),
    (db, tx, degree, bytes): (&Db, TxId, usize, usize),
) -> Computed {
    let count = groups.len();
    let shared: Arc<Vec<Mutex<Vec<Held>>>> = Arc::new(groups.into_iter().map(Mutex::new).collect());
    let (held, db_for) = (Arc::clone(&shared), db.clone());
    let work = Box::new(move |g: usize| -> Result<Vec<Built>> {
        let group = held.get(g).ok_or_else(|| crate::exec::broken("a window's group run twice"))?;
        let rows = group.lock().unwrap_or_else(PoisonError::into_inner);
        computed(&w, &prepared, &rows, (bytes, &Runner::new(&db_for, tx)))
    });
    let built: Result<Vec<Vec<Built>>> = Morsels::start(db, tx, (count, degree), work).collect();
    let taken = shared.iter().map(|g| std::mem::take(&mut *g.lock().unwrap_or_else(PoisonError::into_inner))).collect();
    (taken, built)
}
