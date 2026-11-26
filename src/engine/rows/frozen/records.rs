//! \[DESIGN\] A load's group of rows given by column (RULES.md D1148): of
//! each column of the table, its values as their type writes them in a
//! record, end to end (`storage/lanes.rs`), written as a group just as the
//! same rows are (`stamped.rs`). Sorted by COLUMNSTORE.ORDER (RULES.md
//! D1145), the rows' places are sorted, by the order columns' values as
//! `ordered` compares them, their ids breaking ties; then each column's
//! records are gathered in that order, once.

use std::cmp::Ordering;

use crate::engine::columns::group::{Group, Ordinal, Pages, Seal};
use crate::engine::rows::options::Places;
use crate::error::Result;
use crate::storage::lanes::Records;
use crate::storage::{Row, RowId};
use crate::value::{DataType, NULLS_SORT_HIGH, Value, sort_cmp};

/// Bits of a byte of a null bitmap.
const BITS: usize = 8;

/// The rows of a load's group: as rows of values, or by column, as records.
#[derive(Clone, Copy)]
pub(in crate::engine::rows) enum Batch<'a> {
    /// Rows of values.
    Rows(&'a [Row]),
    /// Of each column of the table, in order, its values' records.
    Records(&'a [Records]),
}

impl Batch<'_> {
    /// How many rows.
    pub(in crate::engine::rows) fn len(&self) -> usize {
        match self {
            Batch::Rows(rows) => rows.len(),
            Batch::Records(columns) => columns.first().map_or(0, Records::len),
        }
    }

    /// Whether it has none.
    pub(in crate::engine::rows) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Of a column the rows are sorted by, its values as they are compared.
enum Keys {
    /// The integers of a type whose record is one ordered as its values
    /// are, none for a null (`Ordinal`).
    Ordinals(Vec<Option<i128>>),
    /// The values, decoded once.
    Values(Vec<Value>),
}

impl Keys {
    /// The keys of `records`, of type `t`: a record that is not one a
    /// null, as it decodes to none.
    fn of(t: &DataType, records: &Records) -> Keys {
        let each = 0..records.len();
        match Ordinal::of(t) {
            Some(o) => Keys::Ordinals(each.map(|n| records.record(n).and_then(|r| o.read(r))).collect()),
            None => Keys::Values(each.map(|n| records.record(n).and_then(|r| t.decode_record(r)).unwrap_or(Value::Null)).collect()),
        }
    }

    /// Row `a`'s against row `b`'s, as `sort_cmp` orders their values.
    fn compare(&self, a: usize, b: usize) -> Ordering {
        match self {
            Keys::Ordinals(keys) => match (keys.get(a).copied().flatten(), keys.get(b).copied().flatten()) {
                (Some(x), Some(y)) => x.cmp(&y),
                (None, None) => Ordering::Equal,
                (None, Some(_)) => null_against_value(),
                (Some(_), None) => null_against_value().reverse(),
            },
            Keys::Values(values) => sort_cmp(values.get(a).unwrap_or(&Value::Null), values.get(b).unwrap_or(&Value::Null)),
        }
    }
}

/// A null against a value not null, as `sort_cmp` orders them.
fn null_against_value() -> Ordering {
    if NULLS_SORT_HIGH { Ordering::Greater } else { Ordering::Less }
}

/// The places of the rows of `columns`, of `types`, whose ids are `ids`,
/// sorted by the columns at `order`, ascending, as `ordered` sorts rows,
/// their ids breaking ties.
pub(super) fn permutation(order: &[usize], types: &[DataType], (ids, columns): (&[RowId], &[Records])) -> Vec<usize> {
    let keys: Vec<Keys> = order.iter().filter_map(|&c| Some(Keys::of(types.get(c)?, columns.get(c)?))).collect();
    let mut places: Vec<usize> = (0..ids.len()).collect();
    places.sort_by(|&a, &b| {
        let by = keys.iter().map(|k| k.compare(a, b)).find(|o| o.is_ne());
        by.unwrap_or_else(|| ids.get(a).cmp(&ids.get(b)))
    });
    places
}

/// The records of `records` at `places`, in that order.
pub(super) fn gathered(records: &Records, places: &[usize]) -> Records {
    let mut out = Records {
        bytes: Vec::with_capacity(records.bytes.len()),
        ends: Vec::with_capacity(places.len()),
        nulls: vec![0; places.len().div_ceil(BITS)],
    };
    for (n, &at) in places.iter().enumerate() {
        if let Some(record) = records.record(at) {
            out.bytes.extend_from_slice(record);
        } else {
            let (byte, bit) = (n.checked_div(BITS).unwrap_or(0), n.checked_rem(BITS).unwrap_or(0));
            if let Some(b) = out.nulls.get_mut(byte) {
                *b |= 1_u8.checked_shl(u32::try_from(bit).unwrap_or(0)).unwrap_or(0);
            }
        }
        out.ends.push(u32::try_from(out.bytes.len()).unwrap_or(u32::MAX));
    }
    out
}

/// Writes the group of `columns`, of `types`, whose ids are `ids`, to
/// `pages`, sorted by the columns at `order` if any, those at `bloom`
/// with Bloom filters and those at `sketch` with sketches: the group of
/// the same rows given as rows (`stamped.rs`).
///
/// # Errors
///
/// As `Group::write_records`.
pub(super) fn group_of_records<S: Seal>(
    pages: &Pages<'_, S>,
    types: &[DataType],
    (ids, columns): (Vec<RowId>, &[Records]),
    (order, bloom, sketch): (&Places, &[usize], &[usize]),
) -> Result<Group> {
    let order = order.places();
    let whole = columns.len() == types.len() && columns.iter().all(|r| r.len() == ids.len());
    if order.is_empty() || !whole {
        return Group::write_records(pages, types, (&ids, columns), (&order, bloom, sketch));
    }
    let places = permutation(&order, types, (&ids, columns));
    let ids: Vec<RowId> = places.iter().filter_map(|&at| ids.get(at).copied()).collect();
    let sorted: Vec<Records> = columns.iter().map(|r| gathered(r, &places)).collect();
    Group::write_records(pages, types, (&ids, &sorted), (&order, bloom, sketch))
}
