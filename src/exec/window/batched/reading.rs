//! \[DESIGN\] RULES.md D560: how a window over batches takes each batch it
//! reads: its calls' arguments evaluated, where its rows begin partitions
//! and peer groups found, cut at the last partition it begins, the rows
//! before added to those held and computed, the rest held; and how it
//! gives way to the rows' form for the rows held and all after them.

use std::sync::Arc;

use super::boundaries::{Last, edges};
use super::compute::computed;
use super::lanes::Built;
use super::spread::{computed_apart, groups};
use super::{Arg, Held, PARTS, Windowing, arguments, prepared, rows_made_batches};
use crate::error::Result;
use crate::exec::batch::vector_of;
use crate::exec::sort::{RunKeys, keys_of, rows_of_batches};
use crate::exec::window::windowed_rows;
use crate::logical::Expr;
use crate::storage::{BATCH_ROWS, Batch, Column, Row};

/// The batches of whole partitions each thread takes, at least, before a
/// window of several threads computes them: enough that a thread's share
/// outweighs handing it out.
const GATHERED: usize = 8;

/// A batch read and its rows' keys, if the sort made them.
type Read = Option<Result<(Batch, Option<RunKeys>)>>;

impl Windowing {
    /// Takes `read`, the next of its input: none at its end.
    ///
    /// # Errors
    ///
    /// The input's.
    pub(super) fn take(&mut self, read: Read) -> Result<()> {
        let Some(read) = read else {
            self.input = None;
            if !self.flushed() {
                self.give_way(None);
            }
            return Ok(());
        };
        let (batch, keys) = read?;
        if batch.rows == 0 {
            return Ok(());
        }
        if self.prepared.is_none() {
            let Ok(made) = prepared(&self.w, &self.runner) else {
                self.give_way(Some(batch));
                return Ok(());
            };
            self.prepared = Some(Arc::new(made));
        }
        let Some(args) = self.arguments(&batch) else {
            self.give_way(Some(batch));
            return Ok(());
        };
        let (partition, both) = (&self.sorted.0, &self.sorted.1);
        let (batch, keys) = match keys.filter(|k| k.ends.len() == both.len()) {
            Some(k) => (batch, Some(k)),
            None => keys_of(both, batch, self.kernels.as_ref()),
        };
        let (parts, peers) = edges((&batch, keys.as_ref()), self.last.as_ref(), (partition, both), self.kernels.as_ref());
        self.last = Last::of(&batch, keys.as_ref());
        let held = Held { batch, args, parts, peers };
        match held.parts.iter().rposition(|&p| p) {
            Some(cut) => {
                let (before, after) = split(held, cut);
                self.hold(before);
                if self.gathered() && !self.flushed() {
                    self.give_way(Some(after.batch));
                    return Ok(());
                }
                self.hold(Some(after));
            }
            None => self.hold(Some(held)),
        }
        if self.bytes > self.limit.checked_div(PARTS).unwrap_or_default() {
            self.give_way(None);
        }
        Ok(())
    }

    /// Of each call, its arguments of `batch`: none if one is not
    /// evaluated by vectors without a warning or an error.
    fn arguments(&self, batch: &Batch) -> Option<Vec<Vec<Arg>>> {
        let arg = |e: &Expr| match e {
            Expr::Column(at) => Some(Arg::At(*at)),
            Expr::Neg(_)
            | Expr::Const(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::SortKey(..)
            | Expr::Domain(..)
            | Expr::Warned(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::NextValue(_)
            | Expr::Invoked(_) => vector_of(e, batch, &self.runner).map(Arg::Made),
        };
        self.w.calls.iter().map(|c| arguments(c).into_iter().map(arg).collect()).collect()
    }

    /// Holds `held`, if any, counting its bytes.
    fn hold(&mut self, held: Option<Held>) {
        let Some(held) = held else { return };
        let made = held.args.iter().flatten().map(|a| if let Arg::Made(c) = a { c.bytes() } else { 0 }).fold(0, usize::saturating_add);
        self.bytes = self.bytes.saturating_add(held.batch.bytes()).saturating_add(made);
        self.held.push(held);
    }

    /// Whether it holds enough whole partitions to compute them: at once
    /// on one thread, else enough for each of its threads to take a share.
    fn gathered(&self) -> bool {
        let rows = self.held.iter().map(|h| h.batch.rows).fold(0, usize::saturating_add);
        self.degree <= 1 || rows >= self.degree.saturating_mul(GATHERED).saturating_mul(BATCH_ROWS)
    }

    /// Computes the calls' values of the rows held, every partition of
    /// them whole, and gives them, beside the batches held: false, the
    /// rows still held, if computing them raises an error, so the window
    /// gives way. Of several threads, groups of whole partitions are
    /// computed on each (`spread.rs`).
    fn flushed(&mut self) -> bool {
        let Some(prepared) = self.prepared.clone().filter(|_| !self.held.is_empty()) else { return true };
        let calls = self.w.calls.len().max(1);
        let trees = self.limit.checked_div(PARTS).unwrap_or_default().checked_div(calls).unwrap_or_default();
        let held = groups(std::mem::take(&mut self.held), self.degree);
        let (held, built) = if held.len() > 1 {
            let shared = (Arc::new(self.w.clone()), prepared);
            computed_apart(held, shared, (&self.db, self.tx, self.degree, trees.checked_div(self.degree).unwrap_or_default()))
        } else {
            let one = held.first().map_or_else(|| Ok(Vec::new()), |h| computed(&self.w, &prepared, h, (trees, &self.runner)));
            (held, one.map(|b| vec![b]))
        };
        let Ok(built) = built else {
            self.held = held.into_iter().flatten().collect();
            return false;
        };
        for (group, values) in held.into_iter().zip(built) {
            self.given(group, &values);
        }
        self.bytes = 0;
        true
    }

    /// Gives the batches of `group`, each with its rows' values of `built`.
    fn given(&mut self, group: Vec<Held>, built: &[Built]) {
        let mut at = 0_usize;
        for held in group {
            let to = at.saturating_add(held.batch.rows);
            let mut batch = held.batch;
            batch.columns.extend(built.iter().map(|b| b.column((at, to))));
            self.out.push_back(batch);
            at = to;
        }
    }

    /// Gives way to the rows' form of the window for the rows held, then
    /// `batch`'s if given, then the rest of its input.
    fn give_way(&mut self, batch: Option<Batch>) {
        let held: Vec<Batch> = std::mem::take(&mut self.held).into_iter().map(|h| h.batch).chain(batch).collect();
        let rest = self.input.take().into_iter().flatten().flat_map(|read| match read {
            Ok((read, _)) => rows_of_batches(&[read]).into_iter().map(Ok).collect::<Vec<Result<Row>>>(),
            Err(e) => vec![Err(e)],
        });
        let rows = Box::new(rows_of_batches(&held).into_iter().map(Ok).chain(rest));
        let given = windowed_rows(self.w.clone(), rows, (&self.db, self.tx));
        self.rows = Some(Box::new(rows_made_batches(given)));
        self.bytes = 0;
    }
}

/// `held` cut before its row `cut`: the rows before, if any, and the rest.
pub(super) fn split(held: Held, cut: usize) -> (Option<Held>, Held) {
    if cut == 0 {
        return (None, held);
    }
    let Held { batch, args, mut parts, mut peers } = held;
    let pick = |from: usize, to: usize| (from..to).map(Some).collect::<Vec<Option<usize>>>();
    let (first, rest) = (pick(0, cut), pick(cut, batch.rows));
    let cut_batch =
        |picks: &[Option<usize>]| Batch { columns: batch.columns.iter().map(|c| c.gathered(picks)).collect(), rows: picks.len() };
    let cut_args = |picks: &[Option<usize>]| -> Vec<Vec<Arg>> {
        args.iter()
            .map(|call| {
                call.iter()
                    .map(|a| match a {
                        Arg::At(at) => Arg::At(*at),
                        Arg::Made(c) => Arg::Made(Column::gathered(c, picks)),
                    })
                    .collect()
            })
            .collect()
    };
    let (after_parts, after_peers) = (parts.split_off(cut), peers.split_off(cut));
    let before = Held { batch: cut_batch(&first), args: cut_args(&first), parts, peers };
    let after = Held { batch: cut_batch(&rest), args: cut_args(&rest), parts: after_parts, peers: after_peers };
    (Some(before), after)
}
