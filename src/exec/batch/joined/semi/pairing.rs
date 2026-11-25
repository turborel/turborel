//! \[DESIGN\] (RULES.md D540, D1070) How a semi join or an antijoin of
//! streams matches a pair of rows beyond their keys: the rest of its
//! condition, judged of a row of the pair's values, the left operand's
//! columns first [ISO/IEC 9075:1992 §7.5 GR1c]. Only the columns the
//! condition reads are put in that row, each from the vector of its
//! operand's batch that holds it, so a pair costs the values it compares.

use crate::error::Result;
use crate::exec::{Runner, broken, eval_cond};
use crate::logical::{Cond, Truth, cond_columns};
use crate::storage::{Batch, Row};
use crate::value::Value;

/// The condition of a join beyond its keys, and of each column it reads,
/// its place in the pair's row, whether it is the right operand's, and
/// the vector of that operand's batches that holds it.
pub(in crate::exec) struct Pairing {
    cond: Option<Cond>,
    reads: Vec<(usize, bool, usize)>,
    width: usize,
}

impl Pairing {
    /// How the pairs of a join on `cond` beyond its keys are matched, of
    /// operands of `widths` whose batches hold `left` and `right`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a column of the condition neither batch holds.
    pub(in crate::exec) fn of(cond: Option<Cond>, (left, right): (&[usize], &[usize]), widths: (usize, usize)) -> Result<Pairing> {
        let mut columns = Vec::new();
        if let Some(c) = &cond {
            cond_columns(c, &mut columns);
        }
        columns.sort_unstable();
        columns.dedup();
        let held = |c: usize, of: &[usize]| {
            of.iter().position(|&h| h == c).ok_or_else(|| broken("a column of a join's condition its operand's batch lacks"))
        };
        let reads = columns
            .into_iter()
            .map(|c| match c.checked_sub(widths.0) {
                Some(r) => Ok((c, true, held(r, right)?)),
                None => Ok((c, false, held(c, left)?)),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Pairing { cond, reads, width: widths.0.saturating_add(widths.1) })
    }

    /// Whether the condition is true of the pair of row `p` of `left`
    /// and row `r` of `right`, of a row of the columns it reads made in
    /// `scratch`; of no condition, true.
    ///
    /// # Errors
    ///
    /// As evaluating the condition fails.
    pub(in crate::exec) fn keeps(
        &self,
        (left, p): (&Batch, usize),
        (right, r): (&Batch, usize),
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<bool> {
        let Some(cond) = &self.cond else { return Ok(true) };
        scratch.resize(self.width, Value::Null);
        for &(slot, of_right, at) in &self.reads {
            if let Some(value) = scratch.get_mut(slot) {
                *value = if of_right { right.cell(at, r).to_value() } else { left.cell(at, p).to_value() };
            }
        }
        Ok(eval_cond(cond, scratch, runner)? == Truth::True)
    }
}
