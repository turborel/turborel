//! \[DESIGN\] What a table holds in memory of each of its frozen row
//! groups (RULES.md D31): its range of row ids, its rows, the columns its
//! rows are sorted by and it keeps Bloom filters of, and of each column
//! its nulls and its least and greatest values, so a scan passes over a
//! group no condition may match without its header, which is read only as
//! it is needed (`columns/cache.rs`). Split from group.rs (RULES.md
//! R-SIZE-2).

use std::mem::size_of;

use super::{Group, Part, Run, Zone, bounds_of};
use crate::disk::pager::PageId;
use crate::storage::{Conjunct, Conjunction, RowId};
use crate::value::Value;

/// \[DESIGN\] A frozen row group, as a table holds it in memory.
#[derive(Debug, Clone)]
pub struct Summary {
    /// The first page of its header.
    pub header: PageId,
    /// Its least row id.
    pub first: RowId,
    /// Its greatest row id.
    pub last: RowId,
    /// How many rows it holds.
    pub rows: u32,
    /// The places of the columns its rows are sorted by.
    pub order: Vec<usize>,
    /// The places of the columns it keeps Bloom filters of.
    pub blooms: Vec<usize>,
    /// Of each column, its nulls and its least and greatest values.
    pub columns: Vec<Overall>,
}

/// Of a column of a group, its nulls, and its least and greatest values
/// that are not null, if any.
#[derive(Debug, Clone)]
pub struct Overall {
    /// How many of its values are null.
    pub nulls: u32,
    /// Its least and greatest values that are not null, if any.
    pub bounds: Option<(Value, Value)>,
}

impl Summary {
    /// What a table holds in memory of `group`, whose header begins at
    /// `header`.
    pub fn of(group: &Group, header: PageId) -> Summary {
        let columns = group
            .columns
            .iter()
            .map(|zones| {
                let ends: Vec<Value> =
                    zones.iter().filter_map(|z| z.bounds.clone()).flat_map(|(least, greatest)| [least, greatest]).collect();
                Overall { nulls: zones.iter().map(|z| z.nulls).fold(0, u32::saturating_add), bounds: bounds_of(&ends).0 }
            })
            .collect();
        Summary {
            header,
            first: group.first,
            last: group.last,
            rows: group.rows,
            order: group.order.clone(),
            blooms: group.blooms(),
            columns,
        }
    }

    /// Whether a row of it may match every one of `conditions`, each of one
    /// column judged by that column's nulls and bounds; every row may if
    /// there are none.
    pub fn may_match(&self, conditions: Option<&Conjunction>) -> bool {
        conditions.is_none_or(|c| c.conjuncts().iter().all(|condition| self.may_match_one(condition)))
    }

    fn may_match_one(&self, condition: &Conjunct) -> bool {
        let columns = condition.columns();
        let [column] = columns.as_slice() else { return true };
        let Some(overall) = self.columns.get(*column) else { return true };
        match condition {
            Conjunct::Null { not: false, .. } => overall.nulls > 0,
            Conjunct::Null { not: true, .. } => overall.nulls < self.rows,
            Conjunct::Condition(_) | Conjunct::In { .. } | Conjunct::NotIn { .. } | Conjunct::Any(_) => {
                let mut bounds: Vec<Option<(Value, Value)>> = vec![None; column.saturating_add(1)];
                if let Some(b) = bounds.get_mut(*column) {
                    b.clone_from(&overall.bounds);
                }
                condition.may_match(&bounds)
            }
        }
    }
}

/// The bytes of the heap a value's own holds besides itself.
fn heap(v: &Value) -> usize {
    match v {
        Value::Char(s) => s.capacity(),
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::Bits(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Extension(_) => 0,
    }
}

/// The bytes of `bounds`, as held.
fn bounds_bytes(bounds: Option<&(Value, Value)>) -> usize {
    bounds.map_or(0, |(least, greatest)| heap(least).saturating_add(heap(greatest)))
}

impl Group {
    /// \[DESIGN\] About the bytes it takes in memory: its structures, and
    /// its bounds' texts.
    pub fn bytes(&self) -> usize {
        let zones: usize = self.columns.iter().flatten().map(|z| size_of::<Zone>().saturating_add(bounds_bytes(z.bounds.as_ref()))).sum();
        let parts: usize = self
            .columns
            .iter()
            .flatten()
            .flat_map(|z| &z.parts)
            .map(|p| size_of::<Part>().saturating_add(bounds_bytes(p.bounds.as_ref())))
            .sum();
        let runs = self.ids.len().saturating_add(self.by_id.len()).saturating_mul(size_of::<Run>());
        size_of::<Group>().saturating_add(zones).saturating_add(parts).saturating_add(runs)
    }
}
