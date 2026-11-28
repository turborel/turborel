//! \[DESIGN\] (RULES.md D1338) The bound the partitions of a grouping
//! share of a sort's first rows picked by set functions (RULES.md
//! D1256). Each partition's thread sorts its own groups' results for
//! their first rows, and gave its sort every group, each one's key
//! written to be compared with the greatest held. The partitions now
//! share one bound of the sort's first key, the bound the morsels of a
//! sort run in parallel share (RULES.md D1241). Each partition's sort
//! offers it the first keys of the rows it holds and raises it once it
//! holds as many as are asked, and each batch of results a partition
//! makes is judged against it before the sort is given it, as a scan
//! judges its rows. A group whose result is past the bound is past the
//! greatest of as many rows as are asked, whichever partitions hold
//! them, and is left out with no key written. One equal to it is kept,
//! as the sort's other keys and the first rows' numbers decide of it.
//!
//! The bound only narrows, and a batch judged by a bound read before
//! another partition narrowed it only keeps more, so the rows the
//! partitions keep between them hold the first of all, in whatever
//! order their threads run. How many rows a partition keeps does
//! differ with that order, which an analyzed plan's count of them
//! shows and nothing a statement returns does.

use std::sync::Arc;

use crate::exec::sort::topped_of;
use crate::index::Probe;
use crate::kernels::Kernels;
use crate::logical::{Most, SortKey};
use crate::storage::bound::{Bound, rescaled_bound};
use crate::storage::{BATCH_ROWS, Column, Condition, Conjunct, Sel, Threshold};

/// \[DESIGN\] (RULES.md D1338) The bound of the first of `keys`, a
/// sort's own, that the partitions share of its first `most` rows, not
/// yet set: of a key whose nulls sort last, as a bound says nothing of
/// nulls, and of no more rows than a batch holds, as a sort run in
/// parallel shares one (`sort::topped_of`, RULES.md D379, D1241).
pub(super) fn shared(keys: &[SortKey], most: usize) -> Option<Arc<Threshold>> {
    let (_, alone) = topped_of(keys, Most { rows: most, ties: false }, None);
    alone.filter(|_| (1..=BATCH_ROWS).contains(&most)).map(|bound| Arc::new(bound.among(most)))
}

/// \[DESIGN\] (RULES.md D1338) The rows of `column`, a batch's results
/// of the sort's first key, that are at or before `bound`, ascending:
/// every row, as none, while the bound holds no value or the column is
/// of a kind not judged here. Counts are judged as the integers they
/// are, by the kernel of `kernels` a scan judges a comparison by, and
/// any other result as its value. A null is left out, as a bound is
/// told only of a key whose nulls sort last.
pub(super) fn within(bound: &Threshold, column: &Column, kernels: &dyn Kernels) -> Option<Sel> {
    let test = bound.row_test(0)?;
    if let Column::Values(values) = column {
        let kept = (0_u32..).zip(values).filter(|&(_, value)| test.matches_key(&[value])).map(|(row, _)| row);
        return Some(Sel::of(kept.collect()));
    }
    let (Column::Ints(ints), Conjunct::Condition(Condition { probe: Probe::Compare { op, value }, .. })) = (column, &test) else {
        return None;
    };
    let mut rows = Vec::new();
    match rescaled_bound(ints.scale, *op, value).filter(|_| ints.runs().is_none())? {
        Bound::Of(folded, unit) => kernels.select_i64(&ints.units, (folded, unit), (ints.nulls.as_ref(), None), &mut rows),
        // True of every row not null: the sort judges each row itself.
        Bound::All => return None,
        Bound::None => {}
    }
    Some(Sel::of(rows))
}

/// \[DESIGN\] (RULES.md D1338) What a partition judges its batches by:
/// the bound the partitions share, if the sort has one, and the kernels
/// of the statement.
pub(super) struct Judge {
    pub(super) bound: Option<Arc<Threshold>>,
    pub(super) kernels: Arc<dyn Kernels>,
}

impl Judge {
    /// The rows of `column`, a batch's results of the sort's first key,
    /// that the sort is given: those at or before the bound (`within`),
    /// or every row, as none.
    pub(super) fn kept(&self, column: Option<&Column>) -> Option<Sel> {
        let (bound, column) = self.bound.as_ref().zip(column)?;
        within(bound, column, &*self.kernels)
    }
}
