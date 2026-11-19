//! The outer rows a LEFT or FULL traversal kept no pair of, each with
//! the nulls of a probed row [ISO/IEC 9075:1992 §7.5 GR3, GR4] (RULES.md
//! D138): every row of the outer table, as the statement reads it, that
//! the operand's filter is true of and whose id no pair recorded. A row
//! of a null geometry is in no tree, and one of an empty geometry pairs
//! only with rows the condition is not true of, so both are among them.

use std::sync::Arc;

use super::super::{IndexJoin, preserved::Ids};
use super::Traversal;
use crate::error::Result;
use crate::storage::{self, Row, Rows};
use crate::txn::checked;

/// The outer table read again, after the pairs.
pub(super) struct Rest {
    kept: Ids,
    rows: Option<Rows>,
}

impl Rest {
    pub(super) fn new(kept: Ids) -> Rest {
        Rest { kept, rows: None }
    }

    /// The next outer row no pair kept, with nulls, as the join gives its
    /// rows; none once the table is read.
    pub(super) fn next(&mut self, traversal: &Traversal, join: &IndexJoin) -> Result<Option<Row>> {
        let outer = &traversal.outer;
        let rows = self.rows.get_or_insert_with(|| {
            let read = Arc::clone(&outer.storage).scan(outer.tx, &traversal.columns);
            Box::new(checked(read, outer.stop.clone()))
        });
        while let Some((id, mut row)) = rows.next().transpose()? {
            if self.kept.contains(id) {
                continue;
            }
            if outer.located {
                row.push(storage::located(id));
            }
            let kept = traversal.operand(row, &join.runner)?;
            if let (Some(mut operand), Some(nulls)) = (kept, &join.nulls) {
                operand.extend(nulls.iter().cloned());
                return join.project(operand).map(Some);
            }
        }
        Ok(None)
    }
}
