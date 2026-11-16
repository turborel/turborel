//! Abstract syntax of the statements of privileges [X3.135-1986 §6.6;
//! ISO/IEC 9075:1992 §11.36, §11.37; ISO/IEC 9075-2:2023 §12.2, §12.3,
//! §12.7]. Split from ast/schema.rs (RULES.md R-SIZE-2).

use super::{DropBehavior, Grantor, RoutineType, TableName, TypeName};

/// [ISO/IEC 9075:1992 §11.36] What an `<object name>` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeObject {
    /// A table or view.
    Table,
    /// A domain.
    Domain,
    /// A collation [ISO/IEC 9075-2:2023 §11.43].
    Collation,
    /// A sequence generator [ISO/IEC 9075-2:2023 §11.72].
    Sequence,
    /// A user-defined type [ISO/IEC 9075-2:2023 §12.3].
    Type,
    /// [ISO/IEC 9075-2:2023 §12.3, §10.6] SPECIFIC `<routine type>`
    /// `<specific name>`.
    SpecificRoutine(RoutineType),
    /// [ISO/IEC 9075-2:2023 §12.3, §10.6] `<routine type>` `<member name>`,
    /// its `<data type list>` if written.
    Routine(RoutineType),
}

/// [ISO/IEC 9075:1992 §11.37] `<revoke statement>` ::= REVOKE \[GRANT
/// OPTION FOR\] `<privileges>` ON `<object name>` FROM `<grantee>` \[{,
/// `<grantee>`}...\] `<drop behavior>`
#[derive(Debug, Clone)]
pub struct RevokeStatement {
    /// GRANT OPTION FOR was written: the grant option goes, the privilege stays.
    pub grant_option_for: bool,
    /// The `<action>`s, or none for ALL PRIVILEGES.
    pub actions: Option<Vec<Action>>,
    /// The `<table name>`, `<domain name>` or `<collation name>`.
    pub table: TableName,
    /// What it names [§11.36].
    pub object: PrivilegeObject,
    /// [ISO/IEC 9075-2:2023 §12.3] Of a routine named by FUNCTION or
    /// ROUTINE, its `<data type list>`, if written.
    pub parameters: Option<Vec<TypeName>>,
    /// The `<grantee>`s: an `<authorization identifier>`, or none for PUBLIC.
    pub grantees: Vec<Option<String>>,
    /// [ISO/IEC 9075-2:2023 §12.7 SR2] GRANTED BY's `<grantor>`, if written.
    pub grantor: Option<Grantor>,
    /// The `<drop behavior>`.
    pub behavior: DropBehavior,
}

/// [X3.135-1986 §6.6] `<privilege definition>` ::= GRANT `<privileges>` ON
/// `<table name>` TO `<grantee>` \[{, `<grantee>`}...\] \[WITH GRANT OPTION\]
#[derive(Debug, Clone)]
pub struct PrivilegeDefinition {
    /// The `<action>`s, or none for ALL PRIVILEGES.
    pub actions: Option<Vec<Action>>,
    /// The `<table name>`, `<domain name>` or `<collation name>`.
    pub table: TableName,
    /// What it names [ISO/IEC 9075:1992 §11.36].
    pub object: PrivilegeObject,
    /// [ISO/IEC 9075-2:2023 §12.3] Of a routine named by FUNCTION or
    /// ROUTINE, its `<data type list>`, if written.
    pub parameters: Option<Vec<TypeName>>,
    /// The `<grantee>`s: an `<authorization identifier>`, or none for PUBLIC.
    pub grantees: Vec<Option<String>>,
    /// WITH GRANT OPTION was written.
    pub grant_option: bool,
    /// [ISO/IEC 9075-2:2023 §12.2 SR6] GRANTED BY's `<grantor>`, if written.
    pub grantor: Option<Grantor>,
}

/// [X3.135-1986 §6.6] `<action>` ::= SELECT | INSERT | DELETE | UPDATE
/// \[(`<grant column list>`)\]; [ISO/IEC 9075:1992 §10.3] INSERT and
/// REFERENCES of a `<privilege column list>`, and USAGE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// SELECT.
    Select,
    /// INSERT, on the columns of the `<privilege column list>`, or on
    /// every column if none is written [ISO/IEC 9075:1992 §10.3 GR4].
    Insert(Option<Vec<String>>),
    /// DELETE.
    Delete,
    /// UPDATE, on the columns of the `<grant column list>`, or on every
    /// column if none is written [X3.135-1986 §6.6 SR2].
    Update(Option<Vec<String>>),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §10.3] REFERENCES, on the columns
    /// listed, or on every column if none is.
    References(Option<Vec<String>>),
    /// [ISO/IEC 9075:1992 §10.3] USAGE, of a domain.
    Usage,
    /// [ISO/IEC 9075-2:2023 §12.3] EXECUTE, of a routine.
    Execute,
    /// [ISO/IEC 9075-2:2023 §12.3] UNDER, of a structured type.
    Under,
    /// [ISO/IEC 9075-2:2023 §12.3] TRIGGER (Feature T217), of a table.
    Trigger,
}
