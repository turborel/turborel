//! Abstract syntax of `<get diagnostics statement>` [ISO/IEC 9075:1992
//! §18.1], with what ISO/IEC 9075-2:2023 §23.1 adds: the `<all
//! information>` of Feature F123 and the items of Feature T511 among
//! others.

use super::{ParameterSpecification, ValueSpecification};

/// \[EXTENSION\] [ISO/IEC 9075:1992 §18.1] `<get diagnostics statement>`
/// ::= GET DIAGNOSTICS `<sql diagnostics information>`
#[derive(Debug, Clone)]
pub enum GetDiagnostics {
    /// `<statement information>`: each `<simple target specification>` =
    /// `<statement information item name>`.
    Statement(Vec<(ParameterSpecification, StatementItem)>),
    /// `<condition information>` ::= EXCEPTION `<condition number>`
    /// `<condition information item>` \[{, ...}\]; [ISO/IEC 9075-2:2023
    /// §23.1] CONDITION in EXCEPTION's place.
    Condition {
        /// The `<condition number>`, a `<simple value specification>`.
        number: ValueSpecification,
        /// Each `<simple target specification>` = `<condition information
        /// item name>`.
        items: Vec<(ParameterSpecification, ConditionItem)>,
    },
    /// [ISO/IEC 9075-2:2023 §23.1] `<all information>` ::= `<all info
    /// target>` = ALL \[`<all qualifier>`\]
    All {
        /// The `<all info target>`, a `<simple target specification>`.
        target: ParameterSpecification,
        /// Its `<all qualifier>`.
        qualifier: AllQualifier,
    },
}

/// [ISO/IEC 9075-2:2023 §23.1] `<all qualifier>`, or its absence.
#[derive(Debug, Clone)]
pub enum AllQualifier {
    /// None written: the statement information and every condition's
    /// [GR6a i, GR6a ii].
    Everything,
    /// STATEMENT.
    Statement,
    /// CONDITION \[`<condition number>`\]: every condition's, or the one
    /// its number names [GR6a ii, GR6a iii].
    Condition(Option<ValueSpecification>),
}

/// [ISO/IEC 9075:1992 §18.1] `<statement information item name>`;
/// [ISO/IEC 9075-2:2023 §23.1] the codes, and Feature T511's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementItem {
    /// NUMBER.
    Number,
    /// MORE.
    More,
    /// COMMAND_FUNCTION.
    CommandFunction,
    /// COMMAND_FUNCTION_CODE.
    CommandFunctionCode,
    /// DYNAMIC_FUNCTION.
    DynamicFunction,
    /// DYNAMIC_FUNCTION_CODE.
    DynamicFunctionCode,
    /// ROW_COUNT.
    RowCount,
    /// TRANSACTIONS_COMMITTED.
    TransactionsCommitted,
    /// TRANSACTIONS_ROLLED_BACK.
    TransactionsRolledBack,
    /// TRANSACTION_ACTIVE.
    TransactionActive,
}

/// [ISO/IEC 9075:1992 §18.1] `<condition information item name>`;
/// [ISO/IEC 9075-2:2023 §23.1] those of a routine, its parameters and a
/// trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionItem {
    /// CATALOG_NAME.
    CatalogName,
    /// CLASS_ORIGIN.
    ClassOrigin,
    /// COLUMN_NAME.
    ColumnName,
    /// CONDITION_NUMBER.
    ConditionNumber,
    /// CONNECTION_NAME.
    ConnectionName,
    /// CONSTRAINT_CATALOG.
    ConstraintCatalog,
    /// CONSTRAINT_NAME.
    ConstraintName,
    /// CONSTRAINT_SCHEMA.
    ConstraintSchema,
    /// CURSOR_NAME.
    CursorName,
    /// MESSAGE_LENGTH.
    MessageLength,
    /// MESSAGE_OCTET_LENGTH.
    MessageOctetLength,
    /// MESSAGE_TEXT.
    MessageText,
    /// PARAMETER_MODE.
    ParameterMode,
    /// PARAMETER_NAME.
    ParameterName,
    /// PARAMETER_ORDINAL_POSITION.
    ParameterOrdinalPosition,
    /// RETURNED_SQLSTATE.
    ReturnedSqlstate,
    /// ROUTINE_CATALOG.
    RoutineCatalog,
    /// ROUTINE_NAME.
    RoutineName,
    /// ROUTINE_SCHEMA.
    RoutineSchema,
    /// SCHEMA_NAME.
    SchemaName,
    /// SERVER_NAME.
    ServerName,
    /// SPECIFIC_NAME.
    SpecificName,
    /// SUBCLASS_ORIGIN.
    SubclassOrigin,
    /// TABLE_NAME.
    TableName,
    /// TRIGGER_CATALOG.
    TriggerCatalog,
    /// TRIGGER_NAME.
    TriggerName,
    /// TRIGGER_SCHEMA.
    TriggerSchema,
}

impl StatementItem {
    /// [ISO/IEC 9075-2:2023 §23.1] Every item, in the order its Format
    /// lists them, which is the order `<all information>` gives them
    /// (US050).
    pub const EVERY: [StatementItem; 10] = [
        StatementItem::Number,
        StatementItem::More,
        StatementItem::CommandFunction,
        StatementItem::CommandFunctionCode,
        StatementItem::DynamicFunction,
        StatementItem::DynamicFunctionCode,
        StatementItem::RowCount,
        StatementItem::TransactionsCommitted,
        StatementItem::TransactionsRolledBack,
        StatementItem::TransactionActive,
    ];

    /// Its `<statement information item name>`.
    pub fn name(self) -> &'static str {
        match self {
            StatementItem::Number => "NUMBER",
            StatementItem::More => "MORE",
            StatementItem::CommandFunction => "COMMAND_FUNCTION",
            StatementItem::CommandFunctionCode => "COMMAND_FUNCTION_CODE",
            StatementItem::DynamicFunction => "DYNAMIC_FUNCTION",
            StatementItem::DynamicFunctionCode => "DYNAMIC_FUNCTION_CODE",
            StatementItem::RowCount => "ROW_COUNT",
            StatementItem::TransactionsCommitted => "TRANSACTIONS_COMMITTED",
            StatementItem::TransactionsRolledBack => "TRANSACTIONS_ROLLED_BACK",
            StatementItem::TransactionActive => "TRANSACTION_ACTIVE",
        }
    }

    /// The item `name` names, if one.
    pub fn named(name: &str) -> Option<StatementItem> {
        StatementItem::EVERY.into_iter().find(|item| item.name() == name)
    }

    /// [ISO/IEC 9075-2:2023 §23.1 Table 37] Whether its data type is exact
    /// numeric with scale 0, rather than a character string.
    pub fn numeric(self) -> bool {
        match self {
            StatementItem::Number
            | StatementItem::CommandFunctionCode
            | StatementItem::DynamicFunctionCode
            | StatementItem::RowCount
            | StatementItem::TransactionsCommitted
            | StatementItem::TransactionsRolledBack
            | StatementItem::TransactionActive => true,
            StatementItem::More | StatementItem::CommandFunction | StatementItem::DynamicFunction => false,
        }
    }
}

impl ConditionItem {
    /// [ISO/IEC 9075-2:2023 §23.1] Every item, in the order its Format
    /// lists them, which is the order `<all information>` gives them
    /// (US050).
    pub const EVERY: [ConditionItem; 27] = [
        ConditionItem::CatalogName,
        ConditionItem::ClassOrigin,
        ConditionItem::ColumnName,
        ConditionItem::ConditionNumber,
        ConditionItem::ConnectionName,
        ConditionItem::ConstraintCatalog,
        ConditionItem::ConstraintName,
        ConditionItem::ConstraintSchema,
        ConditionItem::CursorName,
        ConditionItem::MessageLength,
        ConditionItem::MessageOctetLength,
        ConditionItem::MessageText,
        ConditionItem::ParameterMode,
        ConditionItem::ParameterName,
        ConditionItem::ParameterOrdinalPosition,
        ConditionItem::ReturnedSqlstate,
        ConditionItem::RoutineCatalog,
        ConditionItem::RoutineName,
        ConditionItem::RoutineSchema,
        ConditionItem::SchemaName,
        ConditionItem::ServerName,
        ConditionItem::SpecificName,
        ConditionItem::SubclassOrigin,
        ConditionItem::TableName,
        ConditionItem::TriggerCatalog,
        ConditionItem::TriggerName,
        ConditionItem::TriggerSchema,
    ];

    /// Its `<condition information item name>`.
    pub fn name(self) -> &'static str {
        match self {
            ConditionItem::CatalogName => "CATALOG_NAME",
            ConditionItem::ClassOrigin => "CLASS_ORIGIN",
            ConditionItem::ColumnName => "COLUMN_NAME",
            ConditionItem::ConditionNumber => "CONDITION_NUMBER",
            ConditionItem::ConnectionName => "CONNECTION_NAME",
            ConditionItem::ConstraintCatalog => "CONSTRAINT_CATALOG",
            ConditionItem::ConstraintName => "CONSTRAINT_NAME",
            ConditionItem::ConstraintSchema => "CONSTRAINT_SCHEMA",
            ConditionItem::CursorName => "CURSOR_NAME",
            ConditionItem::MessageLength => "MESSAGE_LENGTH",
            ConditionItem::MessageOctetLength => "MESSAGE_OCTET_LENGTH",
            ConditionItem::MessageText => "MESSAGE_TEXT",
            ConditionItem::ParameterMode => "PARAMETER_MODE",
            ConditionItem::ParameterName => "PARAMETER_NAME",
            ConditionItem::ParameterOrdinalPosition => "PARAMETER_ORDINAL_POSITION",
            ConditionItem::ReturnedSqlstate => "RETURNED_SQLSTATE",
            ConditionItem::RoutineCatalog => "ROUTINE_CATALOG",
            ConditionItem::RoutineName => "ROUTINE_NAME",
            ConditionItem::RoutineSchema => "ROUTINE_SCHEMA",
            ConditionItem::SchemaName => "SCHEMA_NAME",
            ConditionItem::ServerName => "SERVER_NAME",
            ConditionItem::SpecificName => "SPECIFIC_NAME",
            ConditionItem::SubclassOrigin => "SUBCLASS_ORIGIN",
            ConditionItem::TableName => "TABLE_NAME",
            ConditionItem::TriggerCatalog => "TRIGGER_CATALOG",
            ConditionItem::TriggerName => "TRIGGER_NAME",
            ConditionItem::TriggerSchema => "TRIGGER_SCHEMA",
        }
    }

    /// The item `name` names, if one.
    pub fn named(name: &str) -> Option<ConditionItem> {
        ConditionItem::EVERY.into_iter().find(|item| item.name() == name)
    }

    /// [ISO/IEC 9075-2:2023 §23.1 Table 38] Whether its data type is exact
    /// numeric with scale 0, rather than a character string.
    pub fn numeric(self) -> bool {
        match self {
            ConditionItem::ConditionNumber
            | ConditionItem::MessageLength
            | ConditionItem::MessageOctetLength
            | ConditionItem::ParameterOrdinalPosition => true,
            ConditionItem::CatalogName
            | ConditionItem::ClassOrigin
            | ConditionItem::ColumnName
            | ConditionItem::ConnectionName
            | ConditionItem::ConstraintCatalog
            | ConditionItem::ConstraintName
            | ConditionItem::ConstraintSchema
            | ConditionItem::CursorName
            | ConditionItem::MessageText
            | ConditionItem::ParameterMode
            | ConditionItem::ParameterName
            | ConditionItem::ReturnedSqlstate
            | ConditionItem::RoutineCatalog
            | ConditionItem::RoutineName
            | ConditionItem::RoutineSchema
            | ConditionItem::SchemaName
            | ConditionItem::ServerName
            | ConditionItem::SpecificName
            | ConditionItem::SubclassOrigin
            | ConditionItem::TableName
            | ConditionItem::TriggerCatalog
            | ConditionItem::TriggerName
            | ConditionItem::TriggerSchema => false,
        }
    }
}
