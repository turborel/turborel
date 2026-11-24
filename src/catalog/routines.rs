//! The routine descriptor of an SQL-invoked routine of an SQL routine body
//! [ISO/IEC 9075-2:2023 §4.35.5, §11.60 GR3] (Feature T321).

use super::{Horizon, QualifiedName, UsageDesc, Who};
pub use crate::ast::{DataAccess, GenericTable, ParameterMode, RoutineKind, TableSemantics};
use crate::value::DataType;
use crate::value::datetime::Now;

/// [ISO/IEC 9075-2:2023 §4.35.5, §4.9.7] The creation timestamp and the
/// last-altered timestamp a routine descriptor, or a method
/// specification descriptor, includes: each the value of CURRENT_TIMESTAMP
/// as the statement that made or last altered it ran [§11.60 GR5], its
/// instant and the SQL-session's time zone displacement then (Feature
/// T011). An SQL-server module's descriptor keeps them too [ISO/IEC
/// 9075-4:2006 draft §19.4 Description 7, 8].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stamps {
    /// The creation timestamp.
    pub created: Now,
    /// The last-altered timestamp: the creation timestamp until a
    /// statement alters what it describes.
    pub last_altered: Now,
}

impl Stamps {
    /// Of a descriptor made at `now`, never altered: both timestamps
    /// `now` [§11.60 GR5].
    pub fn at(now: Now) -> Stamps {
        Stamps { created: now, last_altered: now }
    }
}

/// [ISO/IEC 9075-2:2023 §4.35.5] An SQL parameter of a routine: its name,
/// its declared type, its ordinal position, its mode and its default.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlParameter {
    /// Its `<SQL parameter name>` \[GR3j iii\].
    pub name: String,
    /// Its declared type \[GR3j i\].
    pub data_type: DataType,
    /// IN, OUT or INOUT; IN of every parameter of a function [§11.60
    /// SR16d] \[GR3j iv\].
    pub mode: ParameterMode,
    /// Of a polymorphic table function, whether it is a generic table
    /// parameter or a descriptor parameter [§4.35.4]: its `data_type` is
    /// then that of the name of its table or descriptor in the function's
    /// component procedures, VARCHAR(128) [§9.26] (IL058).
    pub generic: Option<GenericParameter>,
    /// The text of its `<parameter default>`, if it has a default value, as
    /// a procedure's parameter may (Features T522 and T523) \[GR3c vi\].
    pub default: Option<String>,
}

/// [ISO/IEC 9075-2:2023 §9.26 SR2a vi] (IL058) The maximum length of a
/// PTF extended name, in characters.
pub const PTF_NAME_LENGTH: u32 = 128;

/// [ISO/IEC 9075-2:2023 §9.26 SR2a x] The length of a PTF component
/// procedure's status parameter, an SQLSTATE value.
pub const PTF_STATUS_LENGTH: u32 = 5;

/// [ISO/IEC 9075-2:2023 §9.26 SR2a vii] VARCHAR(L): the declared type of a
/// PTF component procedure's parameter that names a PTF descriptor area or
/// PTF cursor, and of a generic table or descriptor parameter.
pub fn ptf_name_type() -> DataType {
    DataType::Varying { length: PTF_NAME_LENGTH, units: crate::value::CharUnits::Characters }
}

/// [ISO/IEC 9075-2:2023 §9.26 SR2a x] CHARACTER(5): the declared type of a
/// PTF component procedure's status parameter.
pub fn ptf_status_type() -> DataType {
    DataType::Character { length: PTF_STATUS_LENGTH, units: crate::value::CharUnits::Characters }
}

/// [ISO/IEC 9075-2:2023 §4.35.4] What a parameter of a polymorphic table
/// function is that is of no `<data type>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericParameter {
    /// A generic table parameter, its pass through and semantics
    /// explicit.
    Table(GenericTable),
    /// A descriptor parameter.
    Descriptor,
}

/// [ISO/IEC 9075-2:2023 §4.35.4, §11.60 GR3] What the routine descriptor
/// of a polymorphic table function has that another's has not.
#[derive(Debug, Clone, PartialEq)]
pub struct PtfDesc {
    /// Its proper result columns' names and types, of a `<table function
    /// column list>`: none if they are found as it is invoked, by its PTF
    /// describe component procedure, or of RETURNS ONLY PASS THROUGH.
    pub columns: Option<Vec<(String, DataType)>>,
    /// Whether it returns only pass-through columns.
    pub only_pass_through: bool,
    /// Its PTF private parameters, each INOUT \[SR10e\].
    pub private: Vec<SqlParameter>,
    /// The specific names of its PTF describe, start, fulfill and finish
    /// component procedures, as written of each.
    pub describe: Option<QualifiedName>,
    /// Its PTF start component procedure's, if written.
    pub start: Option<QualifiedName>,
    /// Its PTF fulfill component procedure's.
    pub fulfill: QualifiedName,
    /// Its PTF finish component procedure's, if written.
    pub finish: Option<QualifiedName>,
}

impl PtfDesc {
    /// The specific names of its component procedures, in the order
    /// describe, start, fulfill and finish, of those it has.
    pub fn components(&self) -> Vec<&QualifiedName> {
        let fulfill = Some(&self.fulfill);
        [self.describe.as_ref(), self.start.as_ref(), fulfill, self.finish.as_ref()].into_iter().flatten().collect()
    }
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
    /// The sequence generators its body draws from, which DROP SEQUENCE
    /// with RESTRICT may not drop [§11.74 SR3a], each once.
    pub sequences: Vec<QualifiedName>,
    /// The EXECUTE privileges on it [§12.3, §11.60 GR1], as a domain's
    /// USAGE are kept.
    pub execute: Vec<UsageDesc>,
    /// \[EXTENSION\] The DEBUG privileges granted on it, kept as its
    /// EXECUTE privileges are; its owner holds DEBUG as the owner of a
    /// domain holds USAGE, by no grant (RULES.md D1000, D1001).
    pub debug: Vec<UsageDesc>,
    /// \[DESIGN\] Its order of creation: one more than the greatest of the
    /// routines there were as it was created, which no two share, as two
    /// routines of one statement share their timestamps (`stamps`).
    pub created: u64,
    /// Its creation and last-altered timestamps [§4.35.5, GR5].
    pub stamps: Stamps,
    /// [ISO/IEC 9075-4:2006 draft §9.24 GR1] The name of the SQL-server
    /// module that includes it, in its schema, if one does: else it is a
    /// schema-level routine.
    pub module: Option<String>,
    /// [ISO/IEC 9075-2:2023 §4.35.1] Of an SQL-invoked method, the type it
    /// is a method of and its kind: its first parameter is then SELF,
    /// unless it is static [§11.60 SR6e].
    pub method: Option<(QualifiedName, crate::ast::MethodKind)>,
    /// Of a polymorphic table function [§4.35.4], what its descriptor has
    /// more.
    pub ptf: Option<Box<PtfDesc>>,
    /// \[EXTENSION\] Of a distance function of VECTOR, what it declares
    /// more (design document §8, RULES.md D93).
    pub distance: Option<Box<DistanceDesc>>,
}

/// \[EXTENSION\] What CREATE VECTOR DISTANCE FUNCTION declares of a
/// function beyond its routine (design document §8, RULES.md D93): its
/// PROPERTIES, its VALID FORMATS, and its body's expression over its two
/// parameters, which a metric of it evaluates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistanceDesc {
    /// SYMMETRIC.
    pub symmetric: bool,
    /// NON_NEGATIVE.
    pub non_negative: bool,
    /// TRIANGLE_INEQUALITY.
    pub triangle: bool,
    /// The element formats' codes it measures; none for every format.
    pub formats: Vec<u8>,
    /// The `<value expression>` of its body, as written.
    pub expression: String,
}

impl SqlRoutineDesc {
    /// [ISO/IEC 9075-4:2006 draft §9.24 GR1] The name of the SQL-server
    /// module it is of, if it is not a schema-level routine.
    pub fn module_name(&self) -> Option<QualifiedName> {
        self.module.as_ref().map(|m| QualifiedName { auth: self.name.auth.clone(), ident: m.clone() })
    }

    /// Whether it is a function.
    pub fn function(&self) -> bool {
        self.kind == RoutineKind::Function
    }

    /// The specific names of the routines its body invokes, its subject
    /// routines, fixed as it was defined [§4.35.2].
    pub fn invokes(&self) -> &[QualifiedName] {
        self.horizon.subjects().unwrap_or_default()
    }

    /// [ISO/IEC 9075-2:2023 §12.3, §11.60 GR1] Whether `who` holds
    /// EXECUTE on it, and whether grantably: as granted an identifier it
    /// enables or PUBLIC \[§4.42.4\], its owner by the grant of `_SYSTEM`
    /// its definition made.
    pub fn execute_of(&self, who: Who<'_>) -> (bool, bool) {
        let granted = self.execute.iter().filter(|u| who.holds(u.grantee.as_deref()));
        granted.fold((false, false), |(_, grantable), u| (true, grantable || u.grantable))
    }

    /// \[EXTENSION\] Whether `who` holds DEBUG on it, and whether
    /// grantably: `owner`, its owner, grantably, else as granted
    /// (RULES.md D1000, D1001).
    pub fn debug_of(&self, who: Who<'_>, owner: &str) -> (bool, bool) {
        super::domains::usage_held(&self.debug, who, owner)
    }

    /// Whether it is a polymorphic table function [§4.35.4].
    pub fn polymorphic(&self) -> bool {
        self.ptf.is_some()
    }

    /// Whether it has a generic table parameter or a descriptor parameter,
    /// which no `<value expression>` is an argument of [§9.18 SR10d ii,
    /// iii].
    pub fn generic(&self) -> bool {
        self.parameters.iter().any(|p| p.generic.is_some())
    }

    /// Its parameters' declared types, in order.
    pub fn parameter_types(&self) -> Vec<DataType> {
        self.parameters.iter().map(|p| p.data_type.clone()).collect()
    }
}
