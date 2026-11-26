//! \[DESIGN\] (RULES.md D1122) LIKE and NOT LIKE a constant pattern judged
//! of a column given late, of how it is stored: a column store's texts
//! coded by a table of symbols judged of their codes, none decoded
//! (`engine/columns/scan/liked.rs`). Where the source cannot judge them so,
//! the column is decoded of the rows judged alone and judged as text
//! (`texts.rs`), a dictionary's entries each once.

use super::Judged;
use super::texts;
use crate::kernels::Kernels;
use crate::storage::sel::rows_of;
use crate::storage::{Column, Late};
use crate::value::LikePattern;

/// The rows of `over`, or of all, the LIKE `pattern`, NOT LIKE if `not`,
/// keeps of `late`, ascending: unknown of a null, so never kept; none if
/// reading fails, the rows then judged one at a time, which reports it.
pub(super) fn late(
    (pattern, not): (Option<&LikePattern>, bool),
    late: &Late,
    over: Option<&[u32]>,
    kernels: &dyn Kernels,
) -> Option<Vec<u32>> {
    // [X3.135-1986 §5.14 GR4] a null pattern is unknown of every row.
    let Some(pattern) = pattern else { return Some(Vec::new()) };
    if let Some(judged) = late.like(pattern, over).ok()? {
        let rows = over.map_or_else(|| rows_of(0..judged.len()), <[u32]>::to_vec);
        return Some(rows.into_iter().zip(judged).filter(|&(_, m)| m == Some(!not)).map(|(r, _)| r).collect());
    }
    let Column::Texts(decoded) = late.resolved(over, kernels).ok()? else { return None };
    texts::kept(&Judged::Like(0, Some(pattern.clone()), not), &decoded, over, kernels)
}
