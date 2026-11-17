//! Reference types and the tables of structured types: [ISO/IEC
//! 9075-2:2023 §6.1] the `<reference type>`, [§11.3] the `<typed table
//! clause>` of a `<table definition>` and its `<typed table element>`s,
//! and [§11.32] the `<referenceable view specification>` of a `<view
//! definition>`, each checked against the Syntax Rules that need no
//! catalog.

use super::broken;
use crate::ast::{ColumnOptions, RefGeneration, TableConstraintKind, TableElement, TableName, TypeName, TypedTable};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;
use crate::parser::user_types::{GENERATED, REF, UNDER};

/// SCOPE, of a `<scope clause>` [ISO/IEC 9075-2:2023 §6.1].
const SCOPE: &str = "SCOPE";
/// OPTIONS, of WITH OPTIONS [§11.3, §11.32].
const OPTIONS: &str = "OPTIONS";
/// SYSTEM, of SYSTEM GENERATED.
const SYSTEM: &str = "SYSTEM";
/// DERIVED, a `<reference generation>`.
const DERIVED: &str = "DERIVED";

/// The `<typed table clause>` of a `<table definition>` read: the clause,
/// and its elements.
pub(in crate::parser) type TypedContents = (Box<TypedTable>, Vec<TableElement>);

impl Parser {
    /// Whether a `<reference type>` begins here: REF before (.
    pub(in crate::parser) fn at_reference_type(&self) -> bool {
        self.is_word(REF) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
    }

    /// [ISO/IEC 9075-2:2023 §6.1] `<reference type>` ::= REF (
    /// `<referenced type>` ) \[`<scope clause>`\], `<scope clause>` ::=
    /// SCOPE `<table name>`.
    pub(in crate::parser) fn reference_type(&mut self) -> Result<TypeName> {
        self.expect_word(REF)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let referenced = self.table_name()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let scope = self.scope_clause()?;
        Ok(TypeName::Reference(referenced, scope))
    }

    /// \[`<scope clause>`\] ::= SCOPE `<table name>`, if written.
    fn scope_clause(&mut self) -> Result<Option<TableName>> {
        if self.accept_word(SCOPE) { self.table_name().map(Some) } else { Ok(None) }
    }

    /// [ISO/IEC 9075-2:2023 §11.3] `<typed table clause>` ::= OF
    /// `<path-resolved user-defined type name>` \[UNDER `<supertable
    /// name>`\] \[`<typed table element list>`\], after OF, of a table
    /// TEMPORARY if `temporary`.
    pub(in crate::parser) fn typed_table_clause(&mut self, temporary: bool) -> Result<TypedContents> {
        let at = self.here();
        let udt = self.table_name()?;
        let under = if self.accept_word(UNDER) { Some(self.table_name()?) } else { None };
        let mut elements = Vec::new();
        if self.accept_delim(delim::LEFT_PAREN) {
            loop {
                elements.push(self.typed_table_element()?);
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        check_typed_table(&elements, (under.is_some(), temporary), at)?;
        Ok((Box::new(TypedTable { udt, under }), elements))
    }

    /// `<typed table element>` ::= `<column options>` | `<table constraint
    /// definition>` | `<self-referencing column specification>`.
    fn typed_table_element(&mut self) -> Result<TableElement> {
        if self.is_word(REF) && self.next_is_kw(kw::IS) {
            return self.self_referencing(RefGeneration::System);
        }
        if self.at_table_constraint() {
            return Ok(TableElement::Constraint(self.table_constraint()?));
        }
        let at = self.here();
        let column = (self.identifier()?, at);
        self.expect_kw(kw::WITH)?;
        self.expect_word(OPTIONS)?;
        // <column option list> ::= [<scope clause>] [<default clause>]
        // [<column constraint definition>...], one at least [SR11g iii].
        let scope = self.scope_clause()?;
        let default = if self.accept_kw(kw::DEFAULT) { Some(self.default_option()?) } else { None };
        let mut constraints = Vec::new();
        while self.at_column_constraint() {
            constraints.push(self.column_constraint()?);
        }
        if scope.is_none() && default.is_none() && constraints.is_empty() {
            return Err(broken("§11.3 SR11g iii", self.here(), "a <column option list> contains a scope, a default or a constraint"));
        }
        Ok(TableElement::Options(ColumnOptions { column, scope, default, constraints }))
    }

    /// `<self-referencing column specification>` ::= REF IS
    /// `<self-referencing column name>` \[`<reference generation>`\], its
    /// generation `implicit` if none is written.
    fn self_referencing(&mut self, implicit: RefGeneration) -> Result<TableElement> {
        self.expect_word(REF)?;
        self.expect_kw(kw::IS)?;
        let at = self.here();
        let name = self.identifier()?;
        let generation = if self.accept_word(SYSTEM) {
            self.expect_word(GENERATED)?;
            RefGeneration::System
        } else if self.accept_kw(kw::USER) {
            self.expect_word(GENERATED)?;
            RefGeneration::User
        } else if self.accept_word(DERIVED) {
            RefGeneration::Derived
        } else {
            implicit
        };
        Ok(TableElement::SelfReferencing(name, generation, at))
    }

    /// [ISO/IEC 9075-2:2023 §11.32] `<referenceable view specification>`
    /// ::= OF `<path-resolved user-defined type name>` \[UNDER `<table
    /// name>`\] \[`<view element list>`\], after OF.
    pub(in crate::parser) fn referenceable_view(&mut self) -> Result<(TypedTable, Vec<TableElement>)> {
        let at = self.here();
        let udt = self.table_name()?;
        let under = if self.accept_word(UNDER) { Some(self.table_name()?) } else { None };
        let mut elements = Vec::new();
        if self.accept_delim(delim::LEFT_PAREN) {
            loop {
                elements.push(self.view_element()?);
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        check_view(&elements, under.is_some(), at)?;
        Ok((TypedTable { udt, under }, elements))
    }

    /// `<view element>` ::= `<self-referencing column specification>` |
    /// `<view column option>`, `<view column option>` ::= `<column name>`
    /// WITH OPTIONS `<scope clause>`. A self-referencing column of no
    /// generation written is USER GENERATED, of the two it may be
    /// [SR23h iv].
    fn view_element(&mut self) -> Result<TableElement> {
        if self.is_word(REF) && self.next_is_kw(kw::IS) {
            return self.self_referencing(RefGeneration::User);
        }
        let at = self.here();
        let column = (self.identifier()?, at);
        self.expect_kw(kw::WITH)?;
        self.expect_word(OPTIONS)?;
        if !self.is_word(SCOPE) {
            return Err(self.syntax("expected SCOPE"));
        }
        let scope = self.scope_clause()?;
        Ok(TableElement::Options(ColumnOptions { column, scope, default: None, constraints: Vec::new() }))
    }
}

/// The self-referencing column specifications of `elements`: where each
/// is, and its generation.
fn self_references(elements: &[TableElement]) -> Vec<(usize, RefGeneration)> {
    elements
        .iter()
        .filter_map(|e| match e {
            TableElement::SelfReferencing(_, generation, at) => Some((*at, *generation)),
            TableElement::Column(_) | TableElement::Constraint(_) | TableElement::Like(_) | TableElement::Options(_) => None,
        })
        .collect()
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11b, SR11c ii, SR11d v, SR11g ii] Of a
/// typed table written at `at`, its `elements`: a self-referencing column
/// specification exactly once unless a subtable, and then none; none of a
/// temporary table; no PRIMARY KEY of a subtable; no two column options of
/// one column.
fn check_typed_table(elements: &[TableElement], (subtable, temporary): (bool, bool), at: usize) -> Result<()> {
    let references = self_references(elements);
    if let (true, Some((written, _))) = (subtable, references.first()) {
        return Err(broken("§11.3 SR11b", *written, "a subtable has no <self-referencing column specification>"));
    }
    if !subtable && references.len() != 1 {
        let second = references.get(1).map_or(at, |(written, _)| *written);
        return Err(broken(
            "§11.3 SR11b",
            second,
            "a typed table that is no subtable has exactly one <self-referencing column specification>",
        ));
    }
    if let (true, Some((written, _))) = (temporary, references.first()) {
        return Err(broken("§11.3 SR11c ii", *written, "a table of a <self-referencing column specification> is not TEMPORARY"));
    }
    let mut columns: Vec<&str> = Vec::new();
    for e in elements {
        match e {
            TableElement::Constraint(c) if subtable && matches!(c.kind, TableConstraintKind::Unique { primary: true, .. }) => {
                return Err(broken("§11.3 SR11d v", c.at, "a subtable has no PRIMARY KEY"));
            }
            TableElement::Options(o) if columns.contains(&o.column.0.as_str()) => {
                return Err(broken("§11.3 SR11g ii", o.column.1, &format!("column {} has two <column options>", o.column.0)));
            }
            TableElement::Options(o) => columns.push(&o.column.0),
            TableElement::Column(_) | TableElement::Constraint(_) | TableElement::Like(_) | TableElement::SelfReferencing(..) => {}
        }
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.32 SR23e, SR23h i to iii] Of a referenceable
/// view written at `at`, its `elements`: a self-referencing column
/// specification unless a subview, and then none; at most one; never
/// SYSTEM GENERATED.
fn check_view(elements: &[TableElement], subview: bool, at: usize) -> Result<()> {
    let references = self_references(elements);
    if let (true, Some((written, _))) = (subview, references.first()) {
        return Err(broken("§11.32 SR23h ii", *written, "a subview has no <self-referencing column specification>"));
    }
    if !subview && references.is_empty() {
        return Err(broken("§11.32 SR23e", at, "a referenceable view that is no subview has a <self-referencing column specification>"));
    }
    if let Some((written, _)) = references.get(1) {
        return Err(broken("§11.32 SR23h i", *written, "a referenceable view has one <self-referencing column specification> at most"));
    }
    if let Some((written, RefGeneration::System)) = references.first() {
        return Err(broken("§11.32 SR23h iii", *written, "the self-referencing column of a view is not SYSTEM GENERATED"));
    }
    Ok(())
}
