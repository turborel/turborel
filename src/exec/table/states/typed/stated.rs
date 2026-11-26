//! \[DESIGN\] (RULES.md D356, D1186) A group's state of a column of
//! states, written as its accumulator writes it. A count, a sum and a mean
//! are written by the accumulator's own code of the slot the column holds,
//! no accumulator made on the heap: a grouping whose groups are the parts
//! of another (D341) writes every group's state as it gives its groups,
//! and made an accumulator of each to do so. An extreme's, and an
//! approximate sum's, which writes none, still go through the accumulator
//! the group would have had.

use super::Typed;
use crate::functions::{ColumnarKind, avg_written, count_written, sum_written};

impl Typed {
    /// Group `g`'s state, written as its accumulator writes it: whether it
    /// has one.
    pub(in crate::exec::table) fn write(&self, g: usize, out: &mut Vec<u8>) -> bool {
        match self {
            Typed::Count(counts) => count_written(i128::from(counts.get(g).copied().unwrap_or(0)), out),
            Typed::Exact(exact) => {
                let count = i128::from(exact.counts.get(g).copied().unwrap_or(0));
                match exact.kind {
                    ColumnarKind::Sum | ColumnarKind::Count => sum_written(exact.summed(g), &exact.result, out),
                    ColumnarKind::Avg => avg_written(exact.summed(g), count, &exact.result, out),
                    ColumnarKind::Min | ColumnarKind::Max => self.boxed_of(g).write_state(out),
                }
            }
            Typed::Approx(_) => self.boxed_of(g).write_state(out),
        }
    }
}
