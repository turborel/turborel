//! [ISO/IEC 9075:1992 §11.5 GR1b v] The warning implicit zero-bit padding
//! of a definition that places in a descriptor the default of a bit string
//! literal shorter than its fixed-length column or domain: "the value of
//! the `<literal>` extended as necessary on the right with 0-valued bits to
//! the length of the subject data type and a completion condition is
//! raised: warning- implicit zero-bit padding". A default is placed in a
//! column descriptor by a `<column definition>` [§11.4 GR5], of a `<table
//! definition>` [§11.3], an `<add column definition>` [§11.11 GR4] or a
//! `<temporary table declaration>` [§13.11], and by a `<set column default
//! clause>` [§11.13 GR2]; in a domain descriptor by a `<domain
//! definition>` [§11.21 GR3] and a `<set domain default clause>` [§11.23
//! GR2]. The descriptor holds the padded value (`db/defaults.rs`); the
//! statement that made it is found to pad by its text and the descriptor
//! it made, of the subject data type, a domain's for a column of one.

use super::State;
use crate::ast::{
    AlterAction, AlterDomainAction, ColumnDefinition, DefaultOption, DomainDefinition, Literal, Schema, SchemaStatement, TableDefinition,
    TableElement,
};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::SqlState;
use crate::value::DataType;

/// Whether `option`, the default of a descriptor of `subject`, is a bit
/// string literal shorter than a fixed-length `subject`.
fn pads(subject: DataType, option: Option<&DefaultOption>) -> bool {
    let (Some(DefaultOption::Literal(Literal::Bit(bits))), DataType::Bit { length }) = (option, subject) else { return false };
    bits.len() < length
}

/// The warning, if one of `columns`, defined in `desc`, has a default that
/// pads.
fn of_columns<'a>(desc: Option<&TableDesc>, mut columns: impl Iterator<Item = &'a ColumnDefinition>) -> Option<SqlState> {
    let desc = desc?;
    let padded =
        columns.any(|c| desc.column(&c.name).and_then(|i| desc.columns.get(i)).is_some_and(|d| pads(d.data_type, c.default.as_ref())));
    padded.then_some(SqlState::ZERO_BIT_PADDING)
}

/// The `<column definition>`s of `t`.
fn columns_of(t: &TableDefinition) -> impl Iterator<Item = &ColumnDefinition> {
    t.elements.iter().filter_map(|e| match e {
        TableElement::Column(c) => Some(c),
        TableElement::Constraint(_) => None,
    })
}

/// [ISO/IEC 9075:1992 §11.4 GR5, §13.11] The warning of `t`, defined as
/// `desc`, if a default of it pads.
pub(super) fn of_table(desc: &TableDesc, t: &TableDefinition) -> Option<SqlState> {
    of_columns(Some(desc), columns_of(t))
}

impl State {
    /// [ISO/IEC 9075:1992 §11.4 GR5, §11.21 GR3, §11.23 GR2] The warning
    /// of `statement`, which defined or altered `object`, as these
    /// definitions hold it, if a default it placed pads.
    pub(super) fn padding_of_statement(&self, statement: &SchemaStatement, object: &QualifiedName) -> Option<SqlState> {
        let padded = match statement {
            SchemaStatement::Table(t) => return of_columns(self.catalog.table(object), columns_of(t)),
            SchemaStatement::Domain(d) => self.catalog.domain(object).is_some_and(|desc| pads(desc.data_type, d.default.as_ref())),
            SchemaStatement::AlterDomain(a) => match &a.action {
                AlterDomainAction::SetDefault(o) => self.catalog.domain(object).is_some_and(|d| pads(d.data_type, Some(o))),
                AlterDomainAction::DropDefault | AlterDomainAction::AddConstraint(_) | AlterDomainAction::DropConstraint(_) => false,
            },
            SchemaStatement::View(_) | SchemaStatement::Grant(_) | SchemaStatement::Revoke(_) | SchemaStatement::Assertion(_) => false,
        };
        padded.then_some(SqlState::ZERO_BIT_PADDING)
    }

    /// [ISO/IEC 9075:1992 §11.11 GR4, §11.13 GR2] The warning of `action`,
    /// which altered `table`, as these definitions hold it, if a default it
    /// placed pads.
    pub(super) fn padding_of_alter(&self, table: &QualifiedName, action: &AlterAction) -> Option<SqlState> {
        let desc = self.catalog.table(table);
        match action {
            AlterAction::AddColumn(c) => of_columns(desc, std::iter::once(c)),
            AlterAction::AlterColumn { column, default, .. } => {
                let typed = desc.and_then(|d| d.column(column).and_then(|i| d.columns.get(i)));
                typed.is_some_and(|c| pads(c.data_type, default.as_ref())).then_some(SqlState::ZERO_BIT_PADDING)
            }
            AlterAction::DropColumn { .. }
            | AlterAction::AddConstraint(_)
            | AlterAction::DropConstraint { .. }
            | AlterAction::Storage(_) => None,
        }
    }

    /// [ISO/IEC 9075:1992 §11.4 GR5, §11.21 GR3] The warning of `schema`,
    /// as these definitions hold it, if a default of a table or domain of
    /// it pads.
    pub(super) fn padding_of_schema(&self, schema: &Schema) -> Option<SqlState> {
        let typed = |d: &DomainDefinition| self.catalog.domain(&qualify(&d.name, &schema.name)).map(|desc| desc.data_type);
        let domains = schema.domains.iter().any(|d| typed(d).is_some_and(|t| pads(t, d.default.as_ref())));
        let tables =
            || schema.tables.iter().any(|t| of_columns(self.catalog.table(&qualify(&t.name, &schema.name)), columns_of(t)).is_some());
        (domains || tables()).then_some(SqlState::ZERO_BIT_PADDING)
    }
}
