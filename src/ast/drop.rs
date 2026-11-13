//! Abstract syntax of what a DROP statement names and its `<drop
//! behavior>` [ISO/IEC 9075:1992 §11.2, §11.18, §11.20, §11.27].

use super::TableName;

/// What a DROP statement names [ISO/IEC 9075:1992 §11.2, §11.18, §11.20,
/// §11.27].
#[derive(Debug, Clone)]
pub enum DropObject {
    /// A base table.
    Table(TableName),
    /// A view.
    View(TableName),
    /// A domain.
    Domain(TableName),
    /// An assertion [§11.35], which DROP names with no `<drop behavior>`.
    Assertion(TableName),
    /// A collation [ISO/IEC 9075-2:2023 §11.44].
    Collation(TableName),
    /// A sequence generator [ISO/IEC 9075-2:2023 §11.74].
    Sequence(TableName),
    /// A user-defined type [ISO/IEC 9075-2:2023 §11.59].
    Type(TableName),
    /// A schema: `<schema name>` ::= \[`<catalog name>` .\] `<unqualified
    /// schema name>` [§5.4].
    Schema {
        /// The `<catalog name>`, if written.
        catalog: Option<String>,
        /// The `<unqualified schema name>`.
        name: String,
        /// Where it begins in the statement.
        at: usize,
    },
}

impl DropObject {
    /// Where its name begins in the statement.
    pub fn at(&self) -> usize {
        match self {
            DropObject::Collation(name)
            | DropObject::Sequence(name)
            | DropObject::Table(name)
            | DropObject::View(name)
            | DropObject::Domain(name)
            | DropObject::Assertion(name)
            | DropObject::Type(name) => name.at,
            DropObject::Schema { at, .. } => *at,
        }
    }
}

/// [ISO/IEC 9075:1992 §11.2] `<drop behavior>` ::= CASCADE | RESTRICT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropBehavior {
    /// CASCADE: what depends on the object is dropped with it.
    Cascade,
    /// RESTRICT: the object is not dropped while anything depends on it.
    Restrict,
}
