//! \[EXTENSION\] Registered XML Schemas [ISO/IEC 9075-14:2006 draft
//! §4.2.6], which Part 14 lets the implementation register as it chooses
//! (RULES.md D106): CREATE XML SCHEMA, DROP XML SCHEMA and what depends on
//! a registered XML Schema. How the document is evaluated and logged is in
//! `xml_schemas/document.rs`. The permanently registered XML Schemas of
//! Table 1 are constants (`catalog::xml_schemas::PERMANENT`): none is
//! registered again or dropped.
//!
//! A registered XML Schema is read from one document. Its `<include>`,
//! `<redefine>` and `<import>` find documents among the registered XML
//! Schemas alone, those the owner of its schema may use
//! (`catalog::xml_schemas::may_use`), as a registered XML Schema an
//! SQL-schema statement names needs [§11.6 AR1a]; nothing is fetched.
//! Those it read are recorded and are what it depends on.

mod according;
mod dependents;
mod document;

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::revoke::Abandoned;
use super::super::snapshot::xml_schemas::read_registered;
use super::super::{State, rule};
use super::Defined;
use crate::ast::{Literal, ValueExpression, ValueSpecification, XmlSchemaDefinition};
use crate::binder::qualify;
use crate::catalog::xml_schemas::{may_use, permanent};
use crate::catalog::{QualifiedName, XmlSchemaDesc, owner_of, sole};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::txn::LockMode;

/// What depends on a registered XML Schema, of the schemas a search
/// takes (`State::xml_schema_dependents`).
#[derive(Default)]
pub(in crate::db) struct XmlSchemaDependents {
    /// The registered XML Schemas that use it, through `<include>`,
    /// `<redefine>` or `<import>`, directly or through others.
    pub(in crate::db) schemas: Vec<QualifiedName>,
    /// The views, constraints, assertions, triggers and routines that
    /// name it or one of `schemas`, which DROP ... CASCADE drops.
    pub(in crate::db) abandoned: Abandoned,
    /// The tables, domains and user-defined types with a column or type of
    /// an XML type of XMLSCHEMA of it or one of `schemas`, and the tables
    /// of a generated column that invokes a routine of one, which nothing
    /// drops.
    pub(in crate::db) typed: Vec<QualifiedName>,
}

impl XmlSchemaDependents {
    /// [EXTENSION] That registered XML Schema `qn` may be dropped, with
    /// `cascade` or not, or have its USAGE revoked, its dependents these.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a column or domain is of an XML type of it, even
    /// with CASCADE, as no column's values are changed by a drop; else,
    /// without `cascade`, if anything depends on it.
    fn refuse(&self, qn: &QualifiedName, cascade: bool) -> Result<()> {
        if let Some(t) = self.typed.first() {
            return Err(rule(format!(
                "[EXTENSION] {t} is of an XML type of registered XML Schema {qn}, so it is not dropped, even with CASCADE"
            )));
        }
        if cascade {
            return Ok(());
        }
        let first = self.schemas.first().map(|s| format!("registered XML Schema {s}"));
        let Some(what) = first.or_else(|| (!self.abandoned.is_empty()).then(|| self.abandoned.first())) else { return Ok(()) };
        Err(rule(format!("[EXTENSION] {what} depends on registered XML Schema {qn}, so it is dropped only with CASCADE")))
    }
}

impl State {
    /// \[EXTENSION\] CREATE XML SCHEMA `x`, its name in schema `schema` if
    /// it names none, its document a literal, as the session writes it
    /// (`xml_schemas/document.rs`): registered, with the registered XML
    /// Schemas its schema's owner may use as its library [Schema1 §4.2],
    /// and depending on those it read.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its schema is not there (SQLSTATE 3F000), or a
    /// registered XML Schema of its name is [ISO/IEC 9075-14:2006 draft
    /// §4.2.6]; SQLSTATE 2200M if its document is no well-formed XML
    /// document, 22000 if it is no schema document or makes no valid
    /// schema (`xml::schema::read`).
    pub(in crate::db) fn create_xml_schema(&mut self, schema: &str, x: &XmlSchemaDefinition) -> Result<Defined> {
        let qn = qualify(&x.name, schema);
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[EXTENSION] there is no schema {0}: CREATE SCHEMA AUTHORIZATION {0} first", qn.auth);
            return Err(rule(msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(x.name.at));
        }
        if permanent(&qn).is_some() || self.catalog.xml_schema(&qn).is_some() {
            let msg = format!("[EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6] registered XML Schema {qn} exists: one name identifies one");
            return Err(rule(msg).or_at(x.name.at));
        }
        let document = logged_document(&x.document)?;
        let defined = &*self.catalog;
        let owner = owner_of(defined, &qn.auth);
        let library: Vec<&XmlSchemaDesc> =
            defined.xml_schemas().into_iter().filter(|d| may_use(defined, &d.name, sole(defined, owner)).is_ok()).collect();
        let location = x.location.clone().unwrap_or_default();
        let (read, uses) = read_registered(&document, &location, &library)?;
        let target_namespace = read.target_namespace().unwrap_or_default().to_owned();
        let mut locks = vec![(QualifiedName::of_schema(&qn.auth), LockMode::Shared)];
        locks.extend(uses.iter().map(|u| (u.clone(), LockMode::Shared)));
        let desc = XmlSchemaDesc { name: qn, target_namespace, location, document, uses, usage: Vec::new(), schema: Some(read) };
        let mut catalog = self.catalog.duplicate();
        catalog.put_xml_schema(desc);
        self.catalog = Arc::from(catalog);
        Ok(Defined { locks, ..Defined::default() })
    }

    /// \[EXTENSION\] DROP XML SCHEMA `qn`: with `cascade`, with what depends
    /// on it (`xml_schema_dependents`), the registered XML Schemas that
    /// use it dropped as it is, and the privileges on all of them.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is permanently registered [ISO/IEC 9075-14:2006
    /// draft §4.2.6] or not registered, or as `XmlSchemaDependents::refuse`.
    pub(in crate::db) fn drop_xml_schema(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if permanent(qn).is_some() {
            return Err(rule(format!(
                "[EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6] {qn} is permanently registered: it is never dropped"
            )));
        }
        if self.catalog.xml_schema(qn).is_none() {
            return Err(rule(format!("[EXTENSION] there is no registered XML Schema {qn}")));
        }
        let dependents = self.xml_schema_dependents(qn, &|_| true);
        dependents.refuse(qn, cascade)?;
        self.drop_with_dependents(qn, &dependents)
    }

    /// Drops registered XML Schema `qn` and `dependents`: what they
    /// abandon, then the registered XML Schemas that use it, then it.
    fn drop_with_dependents(&mut self, qn: &QualifiedName, dependents: &XmlSchemaDependents) -> Result<Dropped> {
        let (mut dropped, _) = self.destroy(Arc::clone(&self.catalog), &dependents.abandoned)?;
        let mut catalog = self.catalog.duplicate();
        for name in dependents.schemas.iter().chain([qn]) {
            catalog.remove_xml_schema(name);
            dropped.names.push(name.clone());
        }
        self.catalog = Arc::from(catalog);
        Ok(dropped)
    }

    /// What depends on registered XML Schema `qn`, of the schemas `within`
    /// takes: the registered XML Schemas of them that use it, and those
    /// that use one of those, of any schema; and what holds an XML type of
    /// XMLSCHEMA of any of these (`xml_schemas/dependents.rs`): columns,
    /// domains and user-defined types, which nothing drops, and routines
    /// of such a parameter or result, with what invokes them. The one
    /// place what depends on one is found: for DROP, and for a REVOKE of
    /// USAGE, the schemas of owners it leaves without USAGE as `within`.
    pub(in crate::db) fn xml_schema_dependents(&self, qn: &QualifiedName, within: &dyn Fn(&str) -> bool) -> XmlSchemaDependents {
        let all = self.catalog.xml_schemas();
        let mut schemas: Vec<QualifiedName> =
            all.iter().filter(|d| within(&d.name.auth) && d.uses.contains(qn)).map(|d| d.name.clone()).collect();
        // A schema's uses hold every one whose document it read, through
        // others too, so of all schemas the first step finds every user;
        // but of some, those of others that use one found go with it.
        // Each found once, so the walk ends with the catalog's schemas.
        let mut next = 0_usize;
        while let Some(used) = schemas.get(next).cloned() {
            let users = all.iter().filter(|d| d.uses.contains(&used) && d.name != *qn && !schemas.contains(&d.name));
            let found: Vec<QualifiedName> = users.map(|d| d.name.clone()).collect();
            schemas.extend(found);
            next = next.saturating_add(1);
        }
        let gone: Vec<QualifiedName> = std::iter::once(qn.clone()).chain(schemas.iter().cloned()).collect();
        let mut typed = self.typed_by_xml_schemas(&gone, within);
        let mut abandoned = self.naming_xml_schemas(&gone, within);
        // A generated column is no definition a drop takes away alone: its
        // table keeps it, as it keeps a column of such a type.
        let generated = std::mem::take(&mut abandoned.columns).into_iter().map(|(table, _)| table);
        typed.extend(generated.collect::<Vec<_>>());
        XmlSchemaDependents { schemas, abandoned, typed }
    }

    /// [ISO/IEC 9075:1992 §11.2 GR5] The registered XML Schemas of schema
    /// `auth`, which is being dropped, each as DROP XML SCHEMA with
    /// CASCADE drops it, but those a drop before took.
    ///
    /// # Errors
    ///
    /// As `drop_xml_schema`.
    pub(in crate::db) fn drop_schema_xml_schemas(&mut self, auth: &str) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for qn in self.schema_xml_schemas(auth) {
            if self.catalog.xml_schema(&qn).is_some() {
                dropped.absorb(self.drop_xml_schema(&qn, true)?);
            }
        }
        Ok(dropped)
    }

    /// The names of the registered XML Schemas of schema `auth`.
    pub(in crate::db) fn schema_xml_schemas(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.xml_schemas().into_iter().filter(|d| d.name.auth == auth).map(|d| d.name.clone()).collect()
    }
}

/// The document of a CREATE XML SCHEMA as the session wrote it, and the
/// log keeps it (`xml_schemas/document.rs`): a binary string literal of
/// its text's UTF-8 octets, read back as that text, octet for octet.
///
/// # Errors
///
/// SQLCODE -910 if it is anything else, which no statement made here is.
fn logged_document(document: &ValueExpression) -> Result<String> {
    let not_written = || SqlError::new(Code::Api, "[DESIGN] an XML Schema document is defined as a literal of its text".to_owned());
    let ValueExpression::Value(ValueSpecification::Literal(Literal::Binary(octets))) = document else { return Err(not_written()) };
    String::from_utf8(octets.bytes().to_vec()).map_err(|_| not_written())
}

#[cfg(test)]
mod tests;
