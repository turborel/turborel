//! [ISO/IEC 9075-2:2023 §11.59] DROP TYPE of a structured type: what
//! RESTRICT refuses beyond what a distinct type's drop does
//! (`user_types/uses.rs`), and the typed tables CASCADE takes.

use super::super::ddl::Dropped;
use super::super::{State, rule};
use super::names_type;
use crate::catalog::{QualifiedName, StructuredDesc};
use crate::error::Result;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.59 SR4a, SR4c to SR4e, SR4f iv, SR4f v]
    /// That nothing but what binds without it references the structured
    /// type `d`, with RESTRICT: it has no proper subtype, no typed table or
    /// view is of it, it has no transform group, no cast is from or to it,
    /// no other type's attribute and no column is of a reference type of
    /// it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if something does.
    pub(in crate::db) fn structured_unreferenced(&self, d: &StructuredDesc) -> Result<()> {
        let qn = &d.name;
        let cascade = "so it is dropped only with CASCADE";
        if let Some(s) = self.proper_subtypes(qn).first() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4c] type {} is a subtype of {qn}, {cascade}", s.name)));
        }
        if let Some(t) = self.typed_of(std::slice::from_ref(qn)).first() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4d] {t} is a typed table of {qn}, {cascade}")));
        }
        self.no_transform_or_cast(qn)?;
        let others = self.catalog.structured_types().into_iter().filter(|t| t.name != *qn);
        if let Some(t) = others.into_iter().find(|t| t.attributes.iter().any(|a| a.defined_by == t.name && names_type(&a.data_type, qn))) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4f v] an attribute of type {} names type {qn}, {cascade}", t.name)));
        }
        for t in self.catalog.tables() {
            if let Some(c) = t.columns.iter().find(|c| names_type(&c.data_type, qn)) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4a] column {}.{} names type {qn}, {cascade}", t.name, c.name)));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.59 SR4e, SR4f iv] That the user-defined
    /// type `qn`, structured or distinct, has no transform group and is
    /// named by no user-defined cast, as DROP TYPE RESTRICT needs.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it has one, or is named by one.
    pub(in crate::db) fn no_transform_or_cast(&self, qn: &QualifiedName) -> Result<()> {
        let cascade = "so it is dropped only with CASCADE";
        if !self.catalog.transforms(qn).is_empty() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4e] type {qn} has transform groups, {cascade}")));
        }
        if let Some(c) = self.catalog.casts().into_iter().find(|c| names_type(&c.source, qn) || names_type(&c.target, qn)) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.59 SR4f iv] the cast from {} to {} names type {qn}, {cascade}",
                c.source, c.target
            )));
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.59 SR4d] The typed tables and views of the
    /// types `types`.
    pub(in crate::db) fn typed_of(&self, types: &[QualifiedName]) -> Vec<QualifiedName> {
        let tables =
            self.catalog.tables().into_iter().filter(|t| t.typed.as_ref().is_some_and(|d| types.contains(&d.udt))).map(|t| t.name.clone());
        let views =
            self.catalog.views().into_iter().filter(|v| v.typed.as_ref().is_some_and(|d| types.contains(&d.udt))).map(|v| v.name.clone());
        tables.chain(views).collect()
    }

    /// [ISO/IEC 9075-2:2023 §11.59 GR7, §12.7 GR12] The typed tables and
    /// views of the types `types`, dropped with CASCADE, as revoking USAGE
    /// on their type takes them, those a drop before took gone already.
    ///
    /// # Errors
    ///
    /// As each drop's.
    pub(in crate::db) fn drop_typed_tables(&mut self, types: &[QualifiedName]) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for t in self.typed_of(types) {
            if self.catalog.table(&t).is_some() {
                dropped.absorb(self.drop_table(&t, true)?);
            } else if self.catalog.view(&t).is_some() {
                dropped.absorb(self.drop_view(&t, true)?);
            }
        }
        Ok(dropped)
    }
}
