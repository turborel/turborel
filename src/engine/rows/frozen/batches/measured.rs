//! \[DESIGN\] (RULES.md D1267) A column of a frozen run given measured.
//! A reader that takes the octet length of a VARCHAR column and never
//! reads its text asks its scan for the column measured. The scan reads
//! each value's length of the chunks it has open for the run
//! (`columns/scan/lengths.rs`) and gives the column late, as its rows'
//! places (`late.rs`), with those lengths beside them. A run it judged
//! by its lengths (`columns/judged/lengths.rs`) gives the lengths taken
//! then (`Prepared::take_lengths`). No text is made,
//! and the column is not decoded unless a reader asks for its values
//! after all, which then reads what the scan would have decoded.
//!
//! A column that cannot be measured is given as it was before: one of
//! another type, as a CHARACTER value is padded as it is read, or one a
//! chunk of which holds no length the scan can read. The tree's rows
//! and a table that freezes none give their text, which the reader
//! measures itself.

use std::sync::Arc;

use crate::engine::columns::group::{Group, Seal};
use crate::engine::columns::scan::{Reading, octets_at};
use crate::error::Result;
use crate::storage::{Column, Late, LateSource};
use crate::value::DataType;

/// Column `c`, of type `t`, of the rows at `places` of `group`,
/// ascending, measured: late, decoded of `source` if it ever is, with
/// the octet length of each row's value. None if the group gives no
/// late column, the column has no type or it cannot be measured.
///
/// # Errors
///
/// As reading a chunk.
pub(super) fn column<S: Seal>(
    source: Option<&Arc<dyn LateSource>>,
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (c, t): (usize, Option<&DataType>),
    places: Arc<[u32]>,
) -> Result<Option<Column>> {
    let (Some(source), Some(t)) = (source, t) else { return Ok(None) };
    // Those the scan took of the run to judge it, if it did, else read now.
    let lengths = match prepared.take_lengths(c, &places) {
        Some(held) => held,
        None => match octets_at(group, (pages, prepared), (c, t), &places)? {
            Some(read) => read,
            None => return Ok(None),
        },
    };
    Ok(Some(Column::Late(Late::new(Arc::clone(source), c, places).measured(lengths))))
}
