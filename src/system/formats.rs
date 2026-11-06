//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.FILE_FORMATS:
//! each kind of file of a database kept in a directory, and how much of it
//! is still in other formats than STORAGE.ENCODINGS and
//! STORAGE.CHECKSUM_KEY name (RULES.md D48, `db/rekey.rs`), an object
//! "associated with INFORMATION_SCHEMA" this implementation adds. The
//! database's owner sees them; any other, none, as MEMORY_RESERVATIONS.

use super::{Seen, SystemTable, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;
use crate::value::Spelled;
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Value};

/// \[EXTENSION\] INFORMATION_SCHEMA.FILE_FORMATS.
pub(super) struct FileFormats;

impl SystemTable for FileFormats {
    fn name(&self) -> &'static str {
        "FILE_FORMATS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![
            text_column(("KIND", 7), true),
            text_column(("IS_SETTLED", 3), true),
            ColumnDesc {
                name: "BEHIND".to_owned(),
                data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
                not_null: true,
                default: None,
                domain: None,
                collation: None,
            },
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let count = |n: u64| Value::Exact(Decimal::new(i128::from(n), 0));
        Ok(seen.state.formats_behind()?.into_iter().map(|f| vec![text(f.kind), yes(f.settled), count(f.behind)]).collect())
    }
}
