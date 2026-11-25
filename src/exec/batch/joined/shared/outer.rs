//! \[DESIGN\] (RULES.md D1076) A LEFT outer join's probe of a table built
//! for a pipeline's morsels: each row of a batch paired with each built
//! row its key matches, as an inner join's probe pairs them, and a row no
//! built row matches paired with none, its right columns null [ISO/IEC
//! 9075:1992 §7.5 GR4, GR5b], in the order of the batch's rows, as the
//! serial join gives them (`joined/probe.rs`). Its pairs are made a batch
//! at a time as an inner join's are (`probing.rs`, RULES.md D1151).

use super::SharedTable;

impl SharedTable {
    /// The width of the operand's rows.
    pub(in crate::exec) fn width(&self) -> usize {
        self.width
    }
}
