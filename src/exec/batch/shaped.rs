//! \[DESIGN\] The steps a morsel's batches are taken through (RULES.md
//! D290, D354): a filter's, judged over the vectors (`judge.rs`), a
//! projection's, of columns moved (`stream::pick`) or of expressions
//! evaluated (`project::computed`), a hash join's probe of a table its
//! built operand made (`joined/shared.rs`), each by the functions the serial
//! stream applies, so a morsel by batch gives the rows its pipeline row by
//! row would, in the order, and raises its error after the rows before it
//! (RULES.md D15).

use std::sync::Arc;

use super::joined::SharedTable;
use super::judge::Judging;
use super::project::{Place, computed, place_of};
use super::stream::{Batches, filtered, pick, picked};
use crate::db::State;
use crate::exec::Runner;
use crate::logical::{Cond, Expr};
use crate::txn::TxId;

/// A step of a pipeline over its batches.
#[derive(Clone)]
pub(in crate::exec) enum Shaped {
    /// [X3.135-1986 §5.21 GR2] the rows the condition is true of kept.
    Filter(Box<Judging>),
    /// [X3.135-1986 §5.25 GR2] the vectors at these places, moved.
    Pick(Vec<usize>),
    /// [X3.135-1986 §5.25 GR2] each expression evaluated of its vectors.
    Compute(Vec<Place>),
    /// [X3.135-1986 §5.20 GR2b] each row joined with each built row whose
    /// keys, of its vectors at these places, equal its own.
    Probe(Arc<SharedTable>, Vec<usize>),
}

/// The step of a probe of `table` by keys of the columns `left` of
/// batches of `layout` of rows `width` wide, and the layout and width
/// after it; none if a key's column is not among the layout's.
pub(in crate::exec) fn probe_step(
    table: &Arc<SharedTable>,
    left: &[usize],
    (layout, width): (&[usize], usize),
) -> Option<(Shaped, Vec<usize>, usize)> {
    let places = left.iter().map(|c| layout.iter().position(|l| l == c)).collect::<Option<Vec<usize>>>()?;
    let (after, joined) = table.joined_layout((layout, width));
    Some((Shaped::Probe(Arc::clone(table), places), after, joined))
}

/// The step of a filter by `cond` over batches of `layout`, the columns of
/// the rows each vector holds; the layout after it is the same.
pub(in crate::exec) fn filter_step(cond: &Cond, layout: &[usize]) -> Shaped {
    Shaped::Filter(Box::new(Judging::of(cond, layout)))
}

/// The step of a projection by `exprs` over batches of `layout`, and the
/// layout after it, of every expression in order; none if one of them is
/// not evaluated over batches (`project::place_of`).
pub(in crate::exec) fn project_step(exprs: &[Expr], layout: &[usize]) -> Option<(Shaped, Vec<usize>)> {
    let after = (0..exprs.len()).collect();
    if let Some(picks) = picked(exprs, layout) {
        return Some((Shaped::Pick(picks), after));
    }
    let places = exprs.iter().map(|e| place_of(e, layout)).collect::<Option<Vec<Place>>>()?;
    Some((Shaped::Compute(places), after))
}

/// `batches` taken through `steps`, the first first, their expressions
/// evaluated for the statement of `tx`.
pub(in crate::exec) fn through(batches: Batches, steps: &[Shaped], (db, tx): (&Arc<State>, TxId)) -> Batches {
    steps.iter().fold(batches, |inner, step| match step {
        Shaped::Filter(judging) => filtered(inner, ((**judging).clone(), Runner::new(db, tx))),
        Shaped::Pick(picks) => {
            let picks = picks.clone();
            Box::new(inner.map(move |kept| kept.map(|(batch, rows)| (pick(batch, &picks), rows))))
        }
        Shaped::Compute(places) => computed(inner, places.clone(), Runner::new(db, tx)),
        Shaped::Probe(table, places) => probed(inner, Arc::clone(table), places.clone()),
    })
}

/// `batches`, each kept row joined with each built row of `table` its
/// keys at `places` match, in order; an error after the pairs before it.
fn probed(batches: Batches, table: Arc<SharedTable>, places: Vec<usize>) -> Batches {
    Box::new(batches.flat_map(move |kept| match kept {
        Ok((batch, rows)) => table.probed((&batch, rows.as_deref()), (&places, None)),
        Err(e) => vec![Err(e)],
    }))
}
