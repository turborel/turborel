//! [ISO/IEC 9075-2:2023 §4.16.1] Period descriptors: a period's name, its
//! start and end columns, and the name of its implicit constraint.

use super::QualifiedName;

/// [ISO/IEC 9075-2:2023 §4.16.1] A period descriptor of an
/// application-time period: its name, the names of its start and end
/// columns, and the name of its period constraint, the implicit CHECK
/// (start < end) [§11.3 SR2e v 2 B].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodDesc {
    /// The period's name.
    pub name: String,
    /// The name of the period start column.
    pub start: String,
    /// The name of the period end column.
    pub end: String,
    /// The name of the period constraint.
    pub constraint: QualifiedName,
}

impl PeriodDesc {
    /// Whether the column named `column` is its start or end column.
    pub fn bounds(&self, column: &str) -> bool {
        self.start == column || self.end == column
    }
}
