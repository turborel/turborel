//! Reserve the small spatial input before retaining it. Fallback keeps
//! both its reservation and every row already pulled until consumed.

use std::sync::Arc;

use super::super::IndexJoin;
use crate::db::State;
use crate::error::Result;
use crate::exec::{RowIter, broken, work::Work};
use crate::functions::ScalarRef;
use crate::geometry::Geometry;
use crate::memory::Reservation;
use crate::spatial::{Bounds, masks::ROWS, packed::Packed};
use crate::spill::row_bytes;
use crate::storage::Row;

// Geometric growth bounds reallocations while every proposed capacity is
// checked against both the work area and the database memory budget.
const GROWTH: usize = 2;

pub(super) struct Held {
    pub(super) rows: Vec<Row>,
    pub(super) reservation: Reservation,
    limit: usize,
    overflow: Option<Row>,
    payload: usize,
    /// Whether reading stopped at the work area's limit, not a refusal.
    outgrown: bool,
}

impl Held {
    pub(super) fn new(db: &Arc<State>) -> Option<Self> {
        let bytes = metadata(ROWS);
        let limit = Work::ceiling_of(db);
        if bytes > limit {
            return None;
        }
        let reservation = db.memory.reserve(bytes, "a bounded spatial join input").ok()?;
        Some(Self { rows: Vec::with_capacity(ROWS), reservation, limit, overflow: None, payload: 0, outgrown: false })
    }

    pub(super) fn read(&mut self, input: &mut RowIter) -> Result<bool> {
        for row in input.by_ref() {
            let row = row?;
            let payload = self.payload.saturating_add(row_bytes(&row));
            let capacity =
                if self.rows.len() == self.rows.capacity() { self.rows.capacity().saturating_mul(GROWTH) } else { self.rows.capacity() };
            let bytes = metadata(capacity).saturating_add(payload);
            if bytes > self.limit {
                self.overflow = Some(row);
                self.outgrown = true;
                return Ok(false);
            }
            let reserved = self.reservation.resize(bytes);
            if reserved.is_err() {
                self.overflow = Some(row);
                return Ok(false);
            }
            self.rows.reserve_exact(capacity.saturating_sub(self.rows.len()));
            self.payload = payload;
            self.rows.push(row);
        }
        Ok(true)
    }

    /// Whether the input outgrew the work area, so it may be partitioned.
    pub(super) fn outgrown(&self) -> bool {
        self.outgrown
    }

    /// Its rows, then the row that did not fit, and its reservation.
    pub(super) fn into_parts(self) -> (Vec<Row>, Option<Row>, Reservation) {
        (self.rows, self.overflow, self.reservation)
    }

    pub(super) fn bounds(&self, column: usize) -> Result<Vec<Option<Bounds>>> {
        self.rows
            .iter()
            .map(|row| {
                let value = row.get(column).ok_or_else(|| broken("a spatial build column is missing"))?;
                if value.is_null() { Ok(None) } else { Geometry::from_value(value).map(|geometry| geometry.bounds()) }
            })
            .collect()
    }

    pub(super) fn resume(self, mut join: IndexJoin) -> RowIter {
        let Self { rows, reservation, overflow, .. } = self;
        let kept = rows.into_iter().map(move |row| {
            // Keep the reservation until the buffered iterator is dropped.
            let _held = &reservation;
            Ok(row)
        });
        join.left = Box::new(kept.chain(overflow.map(Ok)).chain(join.left));
        Box::new(join)
    }
}

// Charge row slots, construction bounds, per-row functions and the full
// hierarchy before growing. Rows' own payloads are charged separately.
pub(super) fn metadata(capacity: usize) -> usize {
    let each = size_of::<Row>().saturating_add(size_of::<Option<Bounds>>()).saturating_add(size_of::<ScalarRef>());
    Packed::bytes(capacity).saturating_add(capacity.saturating_mul(each)).saturating_add(size_of::<Held>())
}
