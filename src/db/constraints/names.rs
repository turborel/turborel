//! The names of constraints [ISO/IEC 9075:1992 §10.6 SR2]: those there
//! are, of every schema or of one, and whether a constraint's schema may
//! hold it.

use super::Pending;
use crate::catalog::{ConstraintDesc, QualifiedName, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError, SqlState};

impl State {
    /// The names of the constraints there are, of every schema: tables',
    /// domains' and assertions [ISO/IEC 9075:1992 §10.6 SR2].
    pub(in crate::db) fn all_constraint_names(&self) -> Vec<QualifiedName> {
        let tables = self.catalog.tables().into_iter().flat_map(|t| t.constraints.iter().map(|c| c.name.clone()));
        let domains = self.catalog.domains().into_iter().flat_map(|d| d.constraints.iter().map(|c| c.name.clone()));
        let assertions = self.catalog.assertions().into_iter().map(|a| a.name.clone());
        tables.chain(domains).chain(assertions).collect()
    }

    /// The names of the constraints of schema `auth` [ISO/IEC 9075:1992
    /// §10.6 SR2], whatever schema their tables are in.
    pub(in crate::db) fn constraint_names(&self, auth: &str) -> Vec<QualifiedName> {
        self.all_constraint_names().into_iter().filter(|n| n.auth == auth).collect()
    }

    /// That the schema of each name of `constraints` and `pending` of
    /// table `of`, written outside a `<schema definition>`, may hold it
    /// (`constraint_schema`).
    ///
    /// # Errors
    ///
    /// As `constraint_schema`.
    pub(in crate::db) fn constraint_schemas(
        &self,
        of: &QualifiedName,
        (constraints, pending): (&[ConstraintDesc], &[Pending]),
    ) -> Result<()> {
        let names = constraints.iter().filter(|c| c.domain.is_none()).map(|c| &c.name).chain(pending.iter().map(|p| &p.name));
        names.into_iter().try_for_each(|name| self.constraint_schema(name, of))
    }

    /// That the schema of constraint `name`, of table or domain `of`, is
    /// one there is [ISO/IEC 9075:1992 §5.4 SR8b, GR18]; and \[EXTENSION\]
    /// owned by the owner of `of`'s, who defines it: a constraint is named
    /// in a schema only by its owner, which SQL-92 does not require
    /// (RULES.md D47).
    ///
    /// # Errors
    ///
    /// SQLCODE -103, SQLSTATE 3F000, if there is no such schema; -104 if
    /// another owns it.
    pub(in crate::db) fn constraint_schema(&self, name: &QualifiedName, of: &QualifiedName) -> Result<()> {
        if !self.catalog.schema_exists(&name.auth) {
            let msg = format!("[ISO/IEC 9075:1992 §5.4 SR8b, GR18] there is no schema {} for constraint {name}", name.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        let owner = owner_of(&*self.catalog, &of.auth);
        if owner_of(&*self.catalog, &name.auth) != owner {
            let msg = format!(
                "[EXTENSION] constraint {name} is in schema {}, which {owner}, defining it, does not own (RULES.md D47)",
                name.auth
            );
            return Err(SqlError::new(Code::Privilege, msg));
        }
        Ok(())
    }
}
