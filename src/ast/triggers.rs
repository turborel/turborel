//! Abstract syntax of the `<trigger definition>` [ISO/IEC 9075-2:2023
//! §11.49] (Features T200, T211 to T216).

use super::{SearchCondition, SqlStatement, TableName};

/// [ISO/IEC 9075-2:2023 §11.49] `<trigger definition>` ::= CREATE TRIGGER
/// `<trigger name>` `<trigger action time>` `<trigger event>` ON `<table
/// name>` \[REFERENCING `<transition table or variable list>`\]
/// `<triggered action>`
#[derive(Debug, Clone)]
pub struct TriggerDefinition {
    /// The `<trigger name>`.
    pub name: TableName,
    /// The `<trigger action time>`.
    pub time: ActionTime,
    /// The `<trigger event>`.
    pub event: TriggerEvent,
    /// The `<table name>` of its subject table \[SR2\].
    pub table: TableName,
    /// The `<transition table or variable list>`, empty if not written.
    pub referencing: Referencing,
    /// FOR EACH ROW or STATEMENT, STATEMENT if neither is written \[SR8\].
    pub level: TriggerLevel,
    /// The `<search condition>` of the `<triggered when clause>`, if
    /// written.
    pub when: Option<SearchCondition>,
    /// That `<search condition>` as written, which the Information Schema
    /// shows [ISO/IEC 9075-11:2006 draft §6.58 Description 10].
    pub when_text: Option<String>,
    /// The `<SQL procedure statement>`s of the `<triggered SQL
    /// statement>`, in order: one, or those of a `<compound triggered SQL
    /// statement>`.
    pub action: Vec<SqlStatement>,
    /// The `<triggered SQL statement>` as written, whose length the
    /// Information Schema may not show whole \[GR4\].
    pub action_text: String,
    /// \[DESIGN\] The `<trigger definition>` as written, alone even in a
    /// `<schema definition>`: what the trigger descriptor keeps, to read
    /// it again.
    pub text: String,
}

/// [ISO/IEC 9075-2:2023 §11.49] `<trigger action time>` ::= BEFORE | AFTER
/// | INSTEAD OF
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTime {
    /// BEFORE (Feature T214).
    Before,
    /// AFTER (Feature T215).
    After,
    /// INSTEAD OF (Feature T213).
    InsteadOf,
}

/// [ISO/IEC 9075-2:2023 §11.49] `<trigger event>` ::= INSERT | DELETE |
/// UPDATE \[OF `<trigger column list>`\]
#[derive(Debug, Clone)]
pub enum TriggerEvent {
    /// INSERT.
    Insert,
    /// DELETE.
    Delete,
    /// UPDATE, with the `<column name>`s of its `<trigger column list>` and
    /// where each is written; none if no list is written.
    Update(Vec<(String, usize)>),
}

/// [ISO/IEC 9075-2:2023 §11.49] FOR EACH { ROW | STATEMENT }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerLevel {
    /// FOR EACH ROW: a row-level trigger.
    Row,
    /// FOR EACH STATEMENT: a statement-level trigger.
    Statement,
}

/// [ISO/IEC 9075-2:2023 §11.49] The names a `<transition table or variable
/// list>` gives, each with where it is written: OLD \[ROW\] \[AS\] `<old
/// transition variable name>`, NEW \[ROW\] \[AS\] `<new transition variable
/// name>`, OLD TABLE \[AS\] `<old transition table name>` and NEW TABLE
/// \[AS\] `<new transition table name>`.
#[derive(Debug, Clone, Default)]
pub struct Referencing {
    /// The `<old transition variable name>`.
    pub old_row: Option<(String, usize)>,
    /// The `<new transition variable name>`.
    pub new_row: Option<(String, usize)>,
    /// The `<old transition table name>`.
    pub old_table: Option<(String, usize)>,
    /// The `<new transition table name>`.
    pub new_table: Option<(String, usize)>,
}
