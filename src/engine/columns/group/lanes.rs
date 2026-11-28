//! \[DESIGN\] (RULES.md D1319) A lengths lane. Beside the chunks of a
//! VARCHAR column a frozen group writes a second run of chunks that hold
//! the octet length of each row's value, a null where the value is null,
//! as integers in the store's own encodings on pages of their own, with
//! zones as any column of integers has. A reader that wants a value's
//! length and not its text (D1267) reads the lane as it reads integers,
//! many thousand rows a page, where the text's own chunks hold a few
//! hundred and give a length only by their entries' ends or by the sum
//! of a value's codes. The lane's bounds say of a part of its rows
//! whether any value is empty, or every one is.
//!
//! The lengths are the ends of the column's records, which the writer
//! holds already. A column of another type has no lane, as a CHARACTER
//! value is padded as it is read and its length is its type's. A group
//! written in the hybrid layout has none either, nor has one written
//! before lanes were. Of such a group a length is read of the text's
//! chunks as before (`scan/lengths.rs`), so every group is read alike
//! whichever wrote it.

use std::borrow::Cow;

use super::zones::write_column;
use super::{Decoded, Group, Pages, Seal, Zone};
use crate::disk::pager::PageId;
use crate::engine::columns::{Fields, padding};
use crate::error::Result;
use crate::storage::lanes::Records;
use crate::value::{DataType, NARROW_EXACT_PRECISION, Spelled};

/// The type a lane's lengths are written as: an exact number of scale 0
/// in a field of 8 bytes, which every integer encoding holds.
pub const LENGTH: DataType = DataType::Exact { precision: NARROW_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };

/// The octet length of each value of `records`, a VARCHAR column's, as
/// the records of a column of `LENGTH`: a null where the value is null.
fn lengths_of(records: &Records) -> Records {
    let mut lengths = Records { bytes: Vec::with_capacity(records.len().saturating_mul(size_of::<i64>())), ..Records::default() };
    lengths.ends.reserve(records.len());
    lengths.nulls.clone_from(&records.nulls);
    let mut before = 0_u32;
    for (n, &end) in records.ends.iter().enumerate() {
        let null = records.nulls.get(n >> 3_u32).is_some_and(|byte| byte & (1 << (n & 7)) != 0);
        if !null {
            lengths.bytes.extend_from_slice(&i64::from(end.saturating_sub(before)).to_le_bytes());
        }
        lengths.ends.push(u32::try_from(lengths.bytes.len()).unwrap_or(u32::MAX));
        before = end;
    }
    lengths
}

/// Writes the lane of each column of `types` that is VARCHAR, `fields`
/// each column's values written, and gives each lane's chunks by its
/// column's place, none of a column that has no lane.
///
/// # Errors
///
/// As writing a column's chunks.
pub(super) fn write<S: Seal>(
    pages: &Pages<'_, S>,
    types: &[DataType],
    fields: &[Fields<'_>],
    written: &mut Vec<PageId>,
) -> Result<Vec<Option<Vec<Zone>>>> {
    let mut lanes = Vec::with_capacity(types.len());
    for (t, column) in types.iter().zip(fields) {
        if padding(t) != Some(None) {
            lanes.push(None);
            continue;
        }
        let lengths = lengths_of(column.records());
        let lane = Fields::from_records(&LENGTH, Cow::Borrowed(&lengths));
        lanes.push(Some(write_column(pages, &LENGTH, (&lane, &Decoded::new(&LENGTH, &lengths), false), written)?));
    }
    Ok(lanes)
}

impl Group {
    /// \[DESIGN\] (RULES.md D1319) The chunks of the lengths lane of column
    /// `column`, if the group was written with one and its header was
    /// read for the column.
    pub fn lane(&self, column: usize) -> Option<&[Zone]> {
        self.lanes.get(column).and_then(Option::as_deref)
    }

    /// The number a lane's chunks are kept by among the chunks a scan
    /// has open (`prepared.rs`): past every column's own.
    pub fn lane_key(&self, column: usize) -> usize {
        self.columns.len().saturating_add(column)
    }
}
