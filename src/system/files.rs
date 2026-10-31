//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.FILES: each
//! file the database holds (RULES.md D56), an object "associated with
//! INFORMATION_SCHEMA" this implementation adds: where it is, its kind,
//! its class of durability, whether its descriptor is open now, and its
//! reads, writes and syncs.
//! The database's owner sees them; any other, none, as
//! MEMORY_RESERVATIONS. Split from system.rs (RULES.md R-SIZE-2).

use std::path::Path;

use super::{Seen, SystemTable, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;
use crate::value::Spelled;
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Value};

/// \[DESIGN\] The longest path shown.
const PATH_LENGTH: u32 = 1024;

/// \[DESIGN\] The length of the longest name of a class of durability,
/// DURABLE AT COMMIT (`FileClass::name`).
const CLASS_LENGTH: u32 = 17;

/// \[EXTENSION\] INFORMATION_SCHEMA.FILES.
pub(super) struct Files;

impl SystemTable for Files {
    fn name(&self) -> &'static str {
        "FILES"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let count = |name: &str| ColumnDesc {
            name: name.to_owned(),
            data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
            not_null: true,
            default: None,
            domain: None,
        };
        vec![
            text_column(("PATH", PATH_LENGTH), true),
            text_column(("KIND", 7), true),
            text_column(("CLASS", CLASS_LENGTH), false),
            text_column(("IS_OPEN", 3), true),
            count("READS"),
            count("WRITES"),
            count("SYNCS"),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let count = |n: u64| Value::Exact(Decimal::new(i128::from(n), 0));
        let shown =
            |p: &Path| text(&p.display().to_string().chars().take(usize::try_from(PATH_LENGTH).unwrap_or(usize::MAX)).collect::<String>());
        Ok(seen
            .state
            .files
            .infos()
            .into_iter()
            .map(|f| {
                vec![
                    shown(&f.path),
                    text(f.kind.name()),
                    f.class.map_or(Value::Null, |c| text(c.name())),
                    yes(f.open),
                    count(f.reads),
                    count(f.writes),
                    count(f.syncs),
                ]
            })
            .collect())
    }
}
