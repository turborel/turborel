//! [ISO/IEC 9075-2:2023 §11.63, §11.64] User-defined casts: CREATE CAST,
//! checked against its function, and DROP CAST, RESTRICT refused while a
//! definition binds only with it, CASCADE dropping each.

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::definitions::Defined;
use super::super::{State, rule};
use super::user_defined;
use crate::ast::{CastDefinition, DataAccess, TypeName};
use crate::catalog::{CastDesc, QualifiedName, owner_of, sole};
use crate::error::{At, Result};
use crate::txn::LockMode;
use crate::value::DataType;

/// [ISO/IEC 9075-2:2023 §11.63 SR5 to SR8] The schema of the user-defined
/// type `t` is, or of a reference type's referenced type, if it is one.
pub(super) fn type_schema(t: &DataType) -> Option<String> {
    if let DataType::Reference(r) = t {
        return Some(r.referenced.name.schema.clone());
    }
    t.udt_name().map(|n| n.schema)
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.63] CREATE CAST `c`, its names in schema
    /// `schema` if it names none: a cast descriptor [GR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for each Syntax Rule it breaks [SR3, SR4, SR9, SR10].
    pub(in crate::db) fn create_cast(&mut self, schema: &str, c: &CastDefinition) -> Result<Defined> {
        let owner = owner_of(&*self.catalog, schema).to_owned();
        let source = self.cast_type(&c.source, (schema, &owner)).or_at(c.at)?;
        let target = self.cast_type(&c.target, (schema, &owner)).or_at(c.at)?;
        if !user_defined(&source) && !user_defined(&target) {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.63 SR4] a user-defined cast is from or to a user-defined type or a reference type")
                .or_at(c.at));
        }
        let schemas: Vec<String> = [&source, &target].into_iter().filter_map(type_schema).collect();
        let owners: Vec<&str> = schemas.iter().map(|s| owner_of(&*self.catalog, s)).collect();
        if let [a, b] = owners.as_slice()
            && a != b
        {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §11.63 SR9] the types of a cast are owned by one, not by {a} and {b}")).or_at(c.at)
            );
        }
        if self.cast_exists(&source, &target) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.63 SR3] there is a cast from {source} to {target} already")).or_at(c.at));
        }
        let f = self.function_designated(&c.function, (schema, &owner), "§11.63 SR10")?;
        let refused = |why: &str| Err(rule(format!("[ISO/IEC 9075-2:2023 §11.63 {why}, as {} is not", f.name)).or_at(c.function.name.at));
        if f.parameter_types() != [source.clone()] {
            return refused(&format!("SR10a] the cast function has one parameter, of {source}"));
        }
        if f.returns.as_ref() != Some(&target) {
            return refused(&format!("SR10b] the cast function returns {target}"));
        }
        if !owners.contains(&owner_of(&*self.catalog, &f.name.auth)) {
            return refused("SR10c] the cast function is of a schema of the types' owner");
        }
        if !f.deterministic {
            return refused("SR10d] the cast function is deterministic");
        }
        if f.access != DataAccess::Contains {
            return refused("SR10e, SR10f] the cast function neither reads nor modifies SQL-data");
        }
        let locks = vec![(f.specific.clone(), LockMode::Shared)];
        let mut catalog = self.catalog.duplicate();
        catalog.put_cast(CastDesc { source, target, function: f.specific, assignment: c.assignment });
        self.catalog = Arc::from(catalog);
        Ok(Defined { locks, ..Defined::default() })
    }

    /// The `<data type>` `t` of a cast, its names in schema `schema`, as its
    /// owner `owner` names it.
    fn cast_type(&self, t: &TypeName, (schema, owner): (&str, &str)) -> Result<DataType> {
        match self.resolve_type(t, (schema, sole(&*self.catalog, owner)))? {
            (_, Some(d)) => Err(rule(format!("[ISO/IEC 9075-2:2023 §11.63 SR1] a cast's type is a <data type>, not domain {}", d.name))),
            (t, None) => Ok(t),
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.63 SR3, §11.51 GR2b] Whether there is a
    /// user-defined cast from `source` to `target`: one created, or of a
    /// distinct type and its source type, which its definition makes.
    pub(in crate::db) fn cast_exists(&self, source: &DataType, target: &DataType) -> bool {
        let distinct = |a: &DataType, b: &DataType| matches!(a, DataType::Distinct(d) if d.source == *b);
        self.catalog.casts().iter().any(|c| c.source == *source && c.target == *target)
            || distinct(source, target)
            || distinct(target, source)
    }

    /// [ISO/IEC 9075-2:2023 §11.64] Drops the cast from `source` to
    /// `target`: with CASCADE, each routine, view, constraint, assertion and
    /// trigger that binds only with it [GR1 to GR7], the generated columns
    /// that do gone already.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such cast \[SR2\], or, not `cascade`,
    /// something binds only with it \[SR7\].
    pub(in crate::db) fn drop_cast(&mut self, source: &DataType, target: &DataType, cascade: bool) -> Result<Dropped> {
        if !self.catalog.casts().iter().any(|c| c.source == *source && c.target == *target) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.64 SR2] there is no user-defined cast from {source} to {target}")));
        }
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        catalog.remove_cast(source, target);
        after.catalog = Arc::from(catalog);
        self.drop_unbinding(&after, cascade, &format!("the cast from {source} to {target}"), "§11.64 SR7")
    }

    /// The definitions `after`, which lack what is dropped, `what`, made
    /// the definitions: with CASCADE, what binds no more without it dropped
    /// too, as the Syntax Rule `cited` refuses with RESTRICT.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if, not `cascade`, something binds only with it.
    pub(in crate::db) fn drop_unbinding(&mut self, after: &State, cascade: bool, what: &str, cited: &str) -> Result<Dropped> {
        let mut abandoned = self.abandoned_in(after, &|_| true);
        if !cascade && !abandoned.is_empty() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 {cited}] {} binds only with {what}, so it is dropped only with CASCADE",
                abandoned.first()
            )));
        }
        // Lost first, as their rows are copied (`generations_without`).
        abandoned.columns.clear();
        let (mut dropped, locks) = self.destroy(Arc::clone(&after.catalog), &abandoned)?;
        dropped.names.extend(locks.into_iter().map(|(name, _): (QualifiedName, LockMode)| name));
        Ok(dropped)
    }
}
