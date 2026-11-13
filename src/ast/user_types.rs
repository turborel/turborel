//! [ISO/IEC 9075-2:2023 §11.51] `<user-defined type definition>`, as
//! written: the options a distinct type takes, and those it refuses read
//! so a Syntax Rule refuses them (RULES.md D69).

use super::{TableName, TypeName};

/// [ISO/IEC 9075-2:2023 §11.51] `<user-defined type definition>` ::=
/// CREATE TYPE `<schema-resolved user-defined type name>` \[`<subtype
/// clause>`\] \[AS `<representation>`\] \[`<user-defined type option
/// list>`\] \[`<method specification list>`\].
#[derive(Debug, Clone)]
pub struct TypeDefinition {
    /// The `<schema-resolved user-defined type name>`.
    pub name: TableName,
    /// UNDER `<supertype name>`, where it is written, if it is.
    pub under: Option<usize>,
    /// AS `<representation>`: a `<predefined type>` or a `<collection
    /// type>`, if written.
    pub representation: Option<TypeName>,
    /// The `<user-defined type option>`s, in order, each where it begins.
    pub options: Vec<(TypeOption, usize)>,
}

/// [ISO/IEC 9075-2:2023 §11.51] `<user-defined type option>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeOption {
    /// `<instantiable clause>`: INSTANTIABLE, or NOT INSTANTIABLE (false).
    Instantiable(bool),
    /// `<finality>`: FINAL, or NOT FINAL (false).
    Final(bool),
    /// `<reference type specification>`: REF USING, REF FROM or REF IS
    /// SYSTEM GENERATED.
    Reference,
    /// `<cast to ref>`: CAST (SOURCE AS REF) WITH `<identifier>`.
    CastToRef,
    /// `<cast to type>`: CAST (REF AS SOURCE) WITH `<identifier>`.
    CastToType,
    /// `<cast to distinct>`: CAST (SOURCE AS DISTINCT) WITH its
    /// `<identifier>`.
    CastToDistinct(String),
    /// `<cast to source>`: CAST (DISTINCT AS SOURCE) WITH its
    /// `<identifier>`.
    CastToSource(String),
}

impl TypeOption {
    /// The option's kind, as SR4 counts each once.
    pub fn kind(&self) -> &'static str {
        match self {
            TypeOption::Instantiable(_) => "<instantiable clause>",
            TypeOption::Final(_) => "<finality>",
            TypeOption::Reference => "<reference type specification>",
            TypeOption::CastToRef => "<cast to ref>",
            TypeOption::CastToType => "<cast to type>",
            TypeOption::CastToDistinct(_) => "<cast to distinct>",
            TypeOption::CastToSource(_) => "<cast to source>",
        }
    }
}
