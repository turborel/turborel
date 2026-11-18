//! Abstract syntax of what a DROP statement names and its `<drop
//! behavior>` [ISO/IEC 9075:1992 §11.2, §11.18, §11.20, §11.27].

use super::{RoutineDesignator, TableName};

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
    /// A trigger [ISO/IEC 9075-2:2023 §11.50], which DROP names with no
    /// `<drop behavior>`.
    Trigger(TableName),
    /// An SQL-invoked routine [ISO/IEC 9075-2:2023 §11.62], by its
    /// `<specific routine designator>`.
    Routine(RoutineDesignator),
    /// An SQL-server module [ISO/IEC 9075-4:2006 draft §9.22].
    Module(TableName),
    /// [ISO/IEC 9075-2:2023 §11.64] A user-defined cast, by its source and
    /// target types, and where it begins.
    Cast(Box<(super::TypeName, super::TypeName)>, usize),
    /// [ISO/IEC 9075-2:2023 §11.66] A user-defined ordering, by its type.
    Ordering(TableName),
    /// [ISO/IEC 9075-2:2023 §11.71] A type's transform group, or all of
    /// them.
    Transform(TableName, Option<String>),
    /// \[EXTENSION\] A property graph (RULES.md D106), and
    /// whether IF EXISTS was written.
    Graph(TableName, bool),
    /// \[EXTENSION\] An embedding model or a vector pipeline.
    Vector(super::VectorObject, TableName),
    /// \[EXTENSION\] A graph type (graph standard §4.3), and whether IF
    /// EXISTS was written.
    GraphType(TableName, bool),
    /// \[EXTENSION\] A GQL graph, typed or open (§4.3), and whether IF
    /// EXISTS was written.
    GqlGraph(TableName, bool),
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
            | DropObject::Type(name)
            | DropObject::Trigger(name)
            | DropObject::Module(name)
            | DropObject::Ordering(name)
            | DropObject::Transform(name, _)
            | DropObject::Graph(name, _)
            | DropObject::Vector(_, name)
            | DropObject::GraphType(name, _)
            | DropObject::GqlGraph(name, _) => name.at,
            DropObject::Cast(_, at) | DropObject::Schema { at, .. } => *at,
            DropObject::Routine(designator) => designator.name.at,
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
