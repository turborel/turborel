//! \[DESIGN\] What a probe of an index is recorded as having read, for a
//! serializable commit's check (RULES.md D29, D134): the condition its
//! entries answer, or, of an index that says which envelopes its search
//! reads, the region of those envelopes in place of the whole table.

use super::scan::asked;
use crate::catalog::KeyShape;
use crate::error::Result;
use crate::index::{Index, Probe};
use crate::storage::{Condition, Region, TableStorage};
use crate::txn::TxId;

/// What a probe read.
pub(in crate::exec) enum Asked {
    /// The rows a condition matches.
    Condition(Condition),
    /// The rows whose envelopes meet a region's.
    Region(Region),
}

impl Asked {
    /// What a probe of `index`, its key `ix`, for `probe` reads: the region
    /// the index names (`Index::read_region`), if its key is one column,
    /// else what `asked` says. A key computed by an expression has no
    /// column whose values a region could be matched with.
    pub(in crate::exec) fn of(index: &dyn Index, ix: &KeyShape, probe: &Probe) -> Asked {
        let column = match ix.columns.as_slice() {
            [column] if !ix.has_expressions() => Some(*column),
            _ => None,
        };
        match column.and_then(|c| index.read_region(probe).map(|envelopes| Region::new(c, envelopes))) {
            Some(region) => Asked::Region(region),
            None => Asked::Condition(asked(ix, probe.clone())),
        }
    }

    /// Tells `storage` that `tx` read it.
    ///
    /// # Errors
    ///
    /// As `TableStorage::read_by_condition`.
    pub(in crate::exec) fn record(&self, storage: &dyn TableStorage, tx: TxId) -> Result<()> {
        match self {
            Asked::Condition(condition) => storage.read_by_condition(tx, condition),
            Asked::Region(region) => storage.read_by_region(tx, region),
        }
    }
}
