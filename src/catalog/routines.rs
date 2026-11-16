//! The routine descriptor of an SQL-invoked routine of an SQL routine body
//! [ISO/IEC 9075-2:2023 §4.35.5, §11.60 GR3] (Feature T321).

use super::{Horizon, QualifiedName, UsageDesc};
pub use crate::ast::{DataAccess, ParameterMode, RoutineKind};
use crate::value::DataType;

/// [ISO/IEC 9075-2:2023 §4.35.5] An SQL parameter of a routine: its name,
/// its declared type, its ordinal position and its mode.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlParameter {
    /// Its `<SQL parameter name>` \[GR3j iii\].
    pub name: String,
    /// Its declared type \[GR3j i\].
    pub data_type: DataType,
    /// IN, OUT or INOUT; IN of every parameter of a function [§11.60
    /// SR16d] \[GR3j iv\].
    pub mode: ParameterMode,
}

/// [ISO/IEC 9075-2:2023 §4.35.5, §11.60 GR3] A routine descriptor, of an
/// SQL-invoked routine whose language is SQL. Its routine authorization
/// identifier is the owner of its schema, and its SQL-path the schema
/// alone [GR3u, GR3v]: neither is kept, since both follow from `name`.
/// Its SQL security characteristic is DEFINER \[SR23b\]; it has no
/// parameter style, is not a null-call procedure [SR9f i] and has at most
/// 0 dynamic result sets [SR9c, Feature T471 none].
#[derive(Debug, Clone)]
pub struct SqlRoutineDesc {
    /// Its `<schema qualified routine name>` \[GR3a\].
    pub name: QualifiedName,
    /// Its `<specific name>`, in its schema \[GR3b\].
    pub specific: QualifiedName,
    /// A function or a procedure \[GR3d\].
    pub kind: RoutineKind,
    /// Its SQL parameters, in order of their ordinal positions \[GR3j\].
    pub parameters: Vec<SqlParameter>,
    /// Of a function, its result type \[GR3m\].
    pub returns: Option<DataType>,
    /// Whether it is deterministic \[GR3h\].
    pub deterministic: bool,
    /// Its SQL-data access indication \[GR3i\].
    pub access: DataAccess,
    /// Of a function, whether it is a null-call function \[GR3o\].
    pub null_call: bool,
    /// \[DESIGN\] Its `<SQL-invoked routine>` as written, which holds the
    /// routine body \[GR3e\]: read again to bind the body, as a trigger's
    /// action is.
    pub text: String,
    /// Its `<SQL routine body>` as written, which ROUTINES shows.
    pub body: String,
    /// The schema of the names in `text` written without one: the
    /// routine's own, in a `<schema definition>`; else the session's
    /// default schema as it was defined.
    pub schema: String,
    /// The tables and views its body names.
    pub reads: Vec<QualifiedName>,
    /// The columns its body sees of each base table it reads, as they were
    /// when it was defined, and the routines its body invokes, its subject
    /// routines [§4.35.2].
    pub horizon: Horizon,
    /// The EXECUTE privileges on it [§12.3, §11.60 GR1], as a domain's
    /// USAGE are kept.
    pub execute: Vec<UsageDesc>,
    /// \[DESIGN\] Its timestamp of creation \[GR3t\], as an order: one more
    /// than the greatest of the routines there were as it was created.
    pub created: u64,
}

impl SqlRoutineDesc {
    /// Whether it is a function.
    pub fn function(&self) -> bool {
        self.kind == RoutineKind::Function
    }

    /// The specific names of the routines its body invokes, its subject
    /// routines, fixed as it was defined [§4.35.2].
    pub fn invokes(&self) -> &[QualifiedName] {
        self.horizon.subjects().unwrap_or_default()
    }

    /// [ISO/IEC 9075-2:2023 §12.3, §11.60 GR1] Whether `auth` holds
    /// EXECUTE on it, and whether grantably: as granted it or PUBLIC, its
    /// owner by the grant of `_SYSTEM` its definition made.
    pub fn execute_of(&self, auth: &str) -> (bool, bool) {
        let granted = self.execute.iter().filter(|u| u.grantee.as_deref().is_none_or(|g| g == auth));
        granted.fold((false, false), |(_, grantable), u| (true, grantable || u.grantable))
    }

    /// Its parameters' declared types, in order.
    pub fn parameter_types(&self) -> Vec<DataType> {
        self.parameters.iter().map(|p| p.data_type.clone()).collect()
    }
}
