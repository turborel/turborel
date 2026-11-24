//! The NOT NULL constraint ALTER COLUMN … SET NOT NULL adds [ISO/IEC
//! 9075-2:2023 §11.15] (RULES.md D399).

use super::Described;
use crate::catalog::{ConstraintAttributes, ConstraintKind};
use crate::error::Result;

impl Described<'_> {
    /// [ISO/IEC 9075-2:2023 §11.15 GR1b, GR1c] Adds a NOT NULL constraint
    /// of the column at `at`, of a name no constraint of the schema has.
    ///
    /// # Errors
    ///
    /// None in practice: one of the implementation's names is free.
    pub(in crate::db) fn not_null(&mut self, at: usize) -> Result<()> {
        self.attributes = ConstraintAttributes::default();
        self.add(None, "NOTNULL", ConstraintKind::NotNull(at))
    }
}
