//! Abstract syntax of ALTER TABLE's actions [ISO/IEC 9075:1992 §11.10;
//! ISO/IEC 9075-2:2023 §11.10].

use super::{
    AlterColumnAction, ColumnDefinition, DropBehavior, PeriodDefinition, PeriodName, QualifiedName, TableConstraint, TableName,
    TableStorage,
};

/// [ISO/IEC 9075:1992 §11.10] `<alter table action>` ::= `<add column
/// definition>` | `<alter column definition>` | `<drop column definition>`
/// | `<add table constraint definition>` | `<drop table constraint
/// definition>`; [ISO/IEC 9075-2:2023 §11.10] | `<add table period
/// definition>` | `<drop table period definition>` | `<add system
/// versioning clause>` | `<drop system versioning clause>`
#[derive(Debug, Clone)]
pub enum AlterAction {
    /// [ISO/IEC 9075:1992 §11.11] `<add column definition>` ::= ADD \[COLUMN\]
    /// `<column definition>`
    AddColumn(ColumnDefinition),
    /// [ISO/IEC 9075:1992 §11.15] `<drop column definition>` ::= DROP
    /// \[COLUMN\] `<column name>` `<drop behavior>`
    DropColumn {
        /// The `<column name>`.
        column: String,
        /// Where it begins in the statement, for the positions of errors.
        at: usize,
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
    /// [ISO/IEC 9075:1992 §11.12] `<alter column definition>` ::= ALTER
    /// \[COLUMN\] `<column name>` `<alter column action>`
    AlterColumn {
        /// The `<column name>`.
        column: String,
        /// Where it begins in the statement, for the positions of errors.
        at: usize,
        /// The `<alter column action>`.
        action: AlterColumnAction,
    },
    /// [ISO/IEC 9075:1992 §11.16] `<add table constraint definition>` ::=
    /// ADD `<table constraint definition>`
    AddConstraint(TableConstraint),
    /// [ISO/IEC 9075-2:2023 §11.27] `<add table period definition>` ::= ADD
    /// `<table period definition>` \[`<add system time period column
    /// list>`\], the column definitions of that list: ADD \[COLUMN\]
    /// `<column definition 1>` ADD \[COLUMN\] `<column definition 2>`.
    AddPeriod(PeriodDefinition, Vec<ColumnDefinition>),
    /// [ISO/IEC 9075-2:2023 §11.28] DROP `<system or application time period
    /// specification>` `<drop behavior>`: the period's name, where PERIOD
    /// begins, and the behavior.
    DropPeriod {
        /// The period's name.
        name: PeriodName,
        /// Where PERIOD begins in the statement.
        at: usize,
        /// RESTRICT or CASCADE.
        behavior: DropBehavior,
    },
    /// [ISO/IEC 9075-2:2023 §11.29] `<add system versioning clause>` ::=
    /// ADD SYSTEM VERSIONING: where ADD begins.
    AddVersioning(usize),
    /// [ISO/IEC 9075-2:2023 §11.30] `<drop system versioning clause>` ::=
    /// DROP SYSTEM VERSIONING `<drop behavior>`.
    DropVersioning {
        /// Where DROP begins in the statement.
        at: usize,
        /// RESTRICT or CASCADE.
        behavior: DropBehavior,
    },
    /// \[DESIGN\] [ISO/IEC 9075-2:2023 §11.44 GR2] Columns of a collation
    /// dropped made of the character set's: read only from the log, as
    /// DROP COLLATION writes it.
    RevertCollation(QualifiedName),
    /// \[DESIGN\] [ISO/IEC 9075-2:2023 §14.10 GR5, GR8] Every row of the
    /// table gone, at once, and its identity column's generator restarted
    /// if `restart`: read only from the log, as TRUNCATE TABLE writes it.
    Truncate {
        /// Whether RESTART IDENTITY was written.
        restart: bool,
    },
    /// [ISO/IEC 9075:1992 §11.17] `<drop table constraint definition>` ::=
    /// DROP CONSTRAINT `<constraint name>` `<drop behavior>`
    DropConstraint {
        /// The `<constraint name>`.
        name: TableName,
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
    /// \[EXTENSION\] `<alter table storage>` ::= SET `<table options>` |
    /// USING `<engine name>` \[WITH `<table options>`\] (RULES.md D41)
    Storage(TableStorage),
}
