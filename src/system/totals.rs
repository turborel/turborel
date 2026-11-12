//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1]
//! INFORMATION_SCHEMA.ASSERTION_TOTALS: the totals each assertion keeps
//! (RULES.md D51, `db/assertions/kept.rs`), an object "associated with
//! INFORMATION_SCHEMA" this implementation adds, as committed: of each form
//! keeping them, its place among the assertion's forms, whether of SUM, AVG,
//! COUNT or of nulls alone, the values counted, the nulls, the sum, and the
//! rows of them kept, which a fold makes one. As ASSERTIONS shows an assertion
//! [§21.2.17], its schema's owner sees them.

use super::{Seen, SystemTable, identifier, text_column};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;
use crate::value::{DataType, MAX_EXACT_PRECISION, Spelled};

/// \[DESIGN\] The longest sum shown: of 128 bits, its sign and point.
pub(crate) const TOTAL_LENGTH: u32 = 41;

/// \[EXTENSION\] INFORMATION_SCHEMA.ASSERTION_TOTALS.
pub(super) struct AssertionTotals;

impl SystemTable for AssertionTotals {
    fn name(&self) -> &'static str {
        "ASSERTION_TOTALS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let number = |name: &str, not_null: bool| ColumnDesc {
            name: name.to_owned(),
            data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
            not_null,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        };
        vec![
            identifier("CONSTRAINT_CATALOG", true),
            identifier("CONSTRAINT_SCHEMA", true),
            identifier("CONSTRAINT_NAME", true),
            number("FORM_NUMBER", true),
            text_column(("KIND", 5), true),
            number("VALUE_COUNT", false),
            number("NULL_COUNT", true),
            text_column(("TOTAL", TOTAL_LENGTH), false),
            number("ROWS_KEPT", true),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        seen.state.assertion_totals(seen.auth)
    }
}
