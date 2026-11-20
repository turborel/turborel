//! \[DESIGN\] The plan of a merge join (RULES.md D303): the rows of
//! `left` and `right`, each in ascending order of its `key` column,
//! paired where the keys are equal and `cond` is true of the pair, the
//! rows of a product its equalities keep [X3.135-1986 §5.20 GR2b, §5.21
//! GR2], or, `outer`, of a LEFT outer join [ISO/IEC 9075:1992 §7.5], each
//! left row no pair keeps with nulls; in the key's order
//! (`exec/join/merge.rs`). The enumeration chooses it
//! (`physical/enumerate/methods.rs`).

use super::{JoinKey, Plan};
use crate::logical::Cond;

/// A merge join.
#[derive(Debug, Clone)]
pub struct MergeJoin {
    /// Whether the left rows that match none are kept, of a LEFT join.
    pub outer: bool,
    /// The first operand, in its key's order.
    pub left: Plan,
    /// The second operand, in its key's order.
    pub right: Plan,
    /// The column of each its rows are ordered and matched by.
    pub key: JoinKey,
    /// The rest of the condition, over the pair's columns.
    pub cond: Option<Cond>,
    /// The degrees of `left` and `right`, the widths of the nulls.
    pub widths: (usize, usize),
    /// The condition, as text.
    pub detail: String,
}
