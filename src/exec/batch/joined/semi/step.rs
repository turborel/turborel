//! \[DESIGN\] (RULES.md D1075) A hashed semi join or antijoin as a step of
//! a pipeline's batches: its right operand built once for every morsel
//! (`joined/shared.rs`), each batch of a morsel keeps the rows a built
//! row matches, or of an antijoin those none matches, selected, no value
//! copied, as the serial join gives its left's batches on (`semi.rs`). A
//! row of a null key matches nothing, so a semi join drops it and an
//! antijoin keeps it. The rows a morsel keeps, and their order, are those
//! the serial join keeps of them.

use std::sync::Arc;

use super::super::super::stream::Batches;
use super::super::SharedTable;
use super::pairing::Pairing;
use crate::exec::Runner;
use crate::exec::table::KeyHashes;
use crate::logical::Cond;
use crate::physical::JoinKey;
use crate::storage::{Row, Sel};

/// A semi join's probe of its built operand, as a step.
pub(in crate::exec) struct SemiStep {
    table: Arc<SharedTable>,
    /// The place of each key's left column in a batch.
    left: Vec<usize>,
    pairing: Pairing,
    anti: bool,
}

impl SemiStep {
    /// The step of the semi join, or of `anti` the antijoin, on `keys`
    /// and `cond` beyond them, of operands of `widths`, probing `table`
    /// from batches of `layout`; none if a batch lacks a column the keys
    /// or the condition read.
    pub(in crate::exec) fn of(
        table: &Arc<SharedTable>,
        (keys, cond, anti): (&[JoinKey], Option<&Cond>, bool),
        (layout, widths): (&[usize], (usize, usize)),
    ) -> Option<SemiStep> {
        let left = keys.iter().map(|k| layout.iter().position(|&c| c == k.left)).collect::<Option<Vec<usize>>>()?;
        let pairing = Pairing::of(cond.cloned(), (layout, table.held_columns()), widths).ok()?;
        Some(SemiStep { table: Arc::clone(table), left, pairing, anti })
    }

    /// `batches`, each with the rows the step keeps of those it came
    /// with, the condition evaluated by `runner`.
    pub(in crate::exec) fn through(self: Arc<Self>, batches: Batches, runner: Runner) -> Batches {
        let (mut hashes, mut scratch) = (KeyHashes::default(), Row::new());
        Box::new(batches.map(move |kept| {
            let (batch, rows) = kept?;
            let given = crate::storage::sel::places_of(rows, batch.rows);
            let found = self.table.found((&batch, &given), (&self.left, &mut hashes), |r, right| {
                self.pairing.keeps((&batch, r), right, (&mut scratch, &runner))
            })?;
            let chosen: Sel = given.into_iter().zip(found).filter(|&(_, found)| found != self.anti).map(|(r, _)| r).collect();
            Ok((batch, Some(chosen)))
        }))
    }
}
