//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.MEMORY_RESERVATIONS:
//! what holds the memory budget's reservations, and their bytes (RULES.md
//! D38), an object "associated with INFORMATION_SCHEMA" this
//! implementation adds: the buffer pools, and the spill areas of the
//! statements spilling. The database's owner sees them; any other, none,
//! as a spill area tells what others' statements do. Split from system.rs
//! (RULES.md R-SIZE-2).

use super::{Seen, SystemTable, text, text_column};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;
use crate::value::Spelled;
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Value};

/// \[DESIGN\] The longest description of what holds a reservation shown.
const OWNER_LENGTH: u32 = 128;

/// \[EXTENSION\] INFORMATION_SCHEMA.MEMORY_RESERVATIONS.
pub(super) struct MemoryReservations;

impl SystemTable for MemoryReservations {
    fn name(&self) -> &'static str {
        "MEMORY_RESERVATIONS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let bytes = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };
        vec![
            text_column(("RESERVED_BY", OWNER_LENGTH), true),
            ColumnDesc { name: "BYTES".to_owned(), data_type: bytes, not_null: true, default: None, domain: None },
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if seen.state.users.owner() != Some(seen.auth) {
            return Ok(Vec::new());
        }
        let shown = |what: &str| text(&what.chars().take(usize::try_from(OWNER_LENGTH).unwrap_or(usize::MAX)).collect::<String>());
        let reservations = seen.state.memory.reservations();
        Ok(reservations
            .iter()
            .map(|(what, bytes)| vec![shown(what), Value::Exact(Decimal::new(i128::try_from(*bytes).unwrap_or(i128::MAX), 0))])
            .collect())
    }
}
