//! [ISO/IEC 9075:1992 §11.9 GR2, §11.19 GR13, §11.34 GR4] The warning of a
//! definition whose `<search condition>` or `<query expression>` the
//! Information Schema cannot show without truncation: one longer than
//! CHARACTER_DATA, which shows it as the null value
//! (`system::definition::LONG`). Of a statement no longer than that, no
//! part is; so it is looked for only in a longer one.

use crate::ast::{
    AlterAction, AlterDomainAction, ColumnConstraintKind, ColumnDefinition, Schema, SchemaStatement, TableConstraint, TableConstraintKind,
    TableDefinition, TableElement,
};
use crate::db::Completion;
use crate::error::SqlState;
use crate::value::MAX_CHARACTER_LENGTH;

/// Whether `text` is longer than the Information Schema shows.
fn long(text: &str) -> bool {
    text.chars().count() > usize::try_from(MAX_CHARACTER_LENGTH).unwrap_or(usize::MAX)
}

/// `done`, the completion of a definition written `text`, with the warning
/// `found` finds in it, if it completed without one.
pub(super) fn warned(done: Completion, text: &str, found: impl FnOnce() -> Option<SqlState>) -> Completion {
    match done {
        Completion::Success if long(text) => found().map_or(Completion::Success, Completion::Warning),
        Completion::Success | Completion::Warning(_) | Completion::NoData => done,
    }
}

/// [§11.9 GR2] The warning of a `<search condition>` `text` too long.
fn condition(text: &str) -> Option<SqlState> {
    long(text).then_some(SqlState::SEARCH_CONDITION_TOO_LONG)
}

/// Of a `<table constraint definition>`.
fn of_constraint(c: &TableConstraint) -> Option<SqlState> {
    if let TableConstraintKind::Check(check) = &c.kind { condition(&check.text) } else { None }
}

/// Of a `<column definition>`'s constraints.
fn of_column(c: &ColumnDefinition) -> Option<SqlState> {
    c.constraints.iter().find_map(|k| if let ColumnConstraintKind::Check(check) = &k.kind { condition(&check.text) } else { None })
}

/// Of a `<table definition>`'s CHECKs.
fn of_table(t: &TableDefinition) -> Option<SqlState> {
    t.elements.iter().find_map(|e| match e {
        TableElement::Column(c) => of_column(c),
        TableElement::Constraint(c) => of_constraint(c),
    })
}

/// [§11.19 GR13] The warning of a `<query expression>` `text` too long.
fn query(text: &str) -> Option<SqlState> {
    long(text).then_some(SqlState::QUERY_EXPRESSION_TOO_LONG)
}

/// The warning a schema statement's texts give, if any.
pub(super) fn of_statement(s: &SchemaStatement) -> Option<SqlState> {
    match s {
        SchemaStatement::Table(t) => of_table(t),
        SchemaStatement::View(v) => query(&v.text),
        SchemaStatement::Domain(d) => d.constraints.iter().find_map(of_constraint),
        SchemaStatement::AlterDomain(a) => {
            if let AlterDomainAction::AddConstraint(c) = &a.action {
                of_constraint(c)
            } else {
                None
            }
        }
        // [§11.34 GR4]
        SchemaStatement::Assertion(a) => condition(&a.check.text),
        SchemaStatement::Grant(_) | SchemaStatement::Revoke(_) => None,
    }
}

/// The warning an `<alter table action>`'s texts give, if any.
pub(super) fn of_alter(a: &AlterAction) -> Option<SqlState> {
    match a {
        AlterAction::AddColumn(c) => of_column(c),
        AlterAction::AddConstraint(c) => of_constraint(c),
        AlterAction::DropColumn { .. } | AlterAction::AlterColumn { .. } | AlterAction::DropConstraint { .. } | AlterAction::Storage(_) => {
            None
        }
    }
}

/// The warning a `<schema definition>`'s texts give, if any: its search
/// conditions', then its queries'.
pub(super) fn of_schema(s: &Schema) -> Option<SqlState> {
    let conditions = s.tables.iter().find_map(of_table);
    let domains = || s.domains.iter().flat_map(|d| &d.constraints).find_map(of_constraint);
    let assertions = || s.assertions.iter().find_map(|a| condition(&a.check.text));
    conditions.or_else(domains).or_else(assertions).or_else(|| s.views.iter().find_map(|v| query(&v.text)))
}
