//! Abstract syntax of `<get diagnostics statement>` [ISO/IEC 9075:1992
//! §18.1]. Split from ast.rs (RULES.md R-SIZE-2).

use super::{ParameterSpecification, ValueSpecification};

/// \[EXTENSION\] [ISO/IEC 9075:1992 §18.1] `<get diagnostics statement>`
/// ::= GET DIAGNOSTICS `<sql diagnostics information>`
#[derive(Debug, Clone)]
pub enum GetDiagnostics {
    /// `<statement information>`: each `<simple target specification>` =
    /// `<statement information item name>`.
    Statement(Vec<(ParameterSpecification, StatementItem)>),
    /// `<condition information>` ::= EXCEPTION `<condition number>`
    /// `<condition information item>` \[{, ...}\]
    Condition {
        /// The `<condition number>`, a `<simple value specification>`.
        number: ValueSpecification,
        /// Each `<simple target specification>` = `<condition information
        /// item name>`.
        items: Vec<(ParameterSpecification, ConditionItem)>,
    },
}

/// [ISO/IEC 9075:1992 §18.1] `<statement information item name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementItem {
    /// NUMBER.
    Number,
    /// MORE.
    More,
    /// COMMAND_FUNCTION.
    CommandFunction,
    /// DYNAMIC_FUNCTION.
    DynamicFunction,
    /// ROW_COUNT.
    RowCount,
}

/// [ISO/IEC 9075:1992 §18.1] `<condition information item name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionItem {
    /// CONDITION_NUMBER.
    ConditionNumber,
    /// RETURNED_SQLSTATE.
    ReturnedSqlstate,
    /// CLASS_ORIGIN.
    ClassOrigin,
    /// SUBCLASS_ORIGIN.
    SubclassOrigin,
    /// SERVER_NAME.
    ServerName,
    /// CONNECTION_NAME.
    ConnectionName,
    /// CONSTRAINT_CATALOG.
    ConstraintCatalog,
    /// CONSTRAINT_SCHEMA.
    ConstraintSchema,
    /// CONSTRAINT_NAME.
    ConstraintName,
    /// CATALOG_NAME.
    CatalogName,
    /// SCHEMA_NAME.
    SchemaName,
    /// TABLE_NAME.
    TableName,
    /// COLUMN_NAME.
    ColumnName,
    /// CURSOR_NAME.
    CursorName,
    /// MESSAGE_TEXT.
    MessageText,
    /// MESSAGE_LENGTH.
    MessageLength,
    /// MESSAGE_OCTET_LENGTH.
    MessageOctetLength,
}

impl StatementItem {
    /// The item `name` names, if one.
    pub fn named(name: &str) -> Option<StatementItem> {
        Some(match name {
            "NUMBER" => StatementItem::Number,
            "MORE" => StatementItem::More,
            "COMMAND_FUNCTION" => StatementItem::CommandFunction,
            "DYNAMIC_FUNCTION" => StatementItem::DynamicFunction,
            "ROW_COUNT" => StatementItem::RowCount,
            _ => return None,
        })
    }

    /// [ISO/IEC 9075:1992 §18.1 Table 21] Whether its data type is exact
    /// numeric with scale 0, rather than a character string.
    pub fn numeric(self) -> bool {
        matches!(self, StatementItem::Number | StatementItem::RowCount)
    }
}

impl ConditionItem {
    /// The item `name` names, if one.
    pub fn named(name: &str) -> Option<ConditionItem> {
        Some(match name {
            "CONDITION_NUMBER" => ConditionItem::ConditionNumber,
            "RETURNED_SQLSTATE" => ConditionItem::ReturnedSqlstate,
            "CLASS_ORIGIN" => ConditionItem::ClassOrigin,
            "SUBCLASS_ORIGIN" => ConditionItem::SubclassOrigin,
            "SERVER_NAME" => ConditionItem::ServerName,
            "CONNECTION_NAME" => ConditionItem::ConnectionName,
            "CONSTRAINT_CATALOG" => ConditionItem::ConstraintCatalog,
            "CONSTRAINT_SCHEMA" => ConditionItem::ConstraintSchema,
            "CONSTRAINT_NAME" => ConditionItem::ConstraintName,
            "CATALOG_NAME" => ConditionItem::CatalogName,
            "SCHEMA_NAME" => ConditionItem::SchemaName,
            "TABLE_NAME" => ConditionItem::TableName,
            "COLUMN_NAME" => ConditionItem::ColumnName,
            "CURSOR_NAME" => ConditionItem::CursorName,
            "MESSAGE_TEXT" => ConditionItem::MessageText,
            "MESSAGE_LENGTH" => ConditionItem::MessageLength,
            "MESSAGE_OCTET_LENGTH" => ConditionItem::MessageOctetLength,
            _ => return None,
        })
    }

    /// [ISO/IEC 9075:1992 §18.1 Table 21] Whether its data type is exact
    /// numeric with scale 0, rather than a character string.
    pub fn numeric(self) -> bool {
        matches!(self, ConditionItem::ConditionNumber | ConditionItem::MessageLength | ConditionItem::MessageOctetLength)
    }
}
