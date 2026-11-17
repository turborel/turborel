//! Table/method privileges [ISO/IEC 9075-2:2023 §4.42.2, §12.2, §12.3,
//! §12.7, §11.57 GR3] (Feature S024): the methods a `<privilege method
//! list>` names, each a method of the table's type the grantor or revoker
//! holds EXECUTE on, and those an overriding method takes over (RULES.md
//! D81). Those SELECT on a typed table grants with it are found as they
//! are read (`catalog::privileges::granted`).

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::{State, rule};
use crate::ast::RoutineDesignator;
use crate::binder::qualify;
use crate::catalog::{QualifiedName, TableDesc, owner_of, sole, type_methods, usage_held};
use crate::error::{At, Code, Result, SqlError};

impl State {
    /// [ISO/IEC 9075-2:2023 §12.3 SR6, SR8b i] The specific names of the
    /// methods the `<privilege method list>` `list` names on `table`, its
    /// names in schema `schema`, each of which `auth` holds EXECUTE on, as
    /// the Access Rule `cited` asks.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table is of no structured type, or a designator
    /// names no method of it [SR6]; -104 if `auth` holds no EXECUTE on one.
    pub(in crate::db) fn privilege_methods(
        &self,
        (auth, schema): (&str, &str),
        table: &TableDesc,
        (list, cited): (&[RoutineDesignator], &str),
    ) -> Result<BTreeSet<QualifiedName>> {
        if list.is_empty() {
            return Ok(BTreeSet::new());
        }
        let Some(typed) = &table.typed else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §12.3 SR6] {} is no table of a structured type, so SELECT names no method of it",
                table.name
            )));
        };
        let of = type_methods(&*self.catalog, &typed.udt);
        let mut found = BTreeSet::new();
        for d in list {
            let specific = self.method_of_list((auth, schema), d, &of).or_at(d.name.at)?;
            if !self.executes_method(auth, &specific) {
                let msg = format!("[ISO/IEC 9075-2:2023 {cited}] {auth} holds no EXECUTE on method {specific}");
                return Err(SqlError::new(Code::Privilege, msg).or_at(d.name.at));
            }
            found.insert(specific);
        }
        Ok(found)
    }

    /// [ISO/IEC 9075-2:2023 §12.3 SR6, §10.6] The specific name of the
    /// method of `of` the designator `d` names: an observer's or a
    /// mutator's by its specific name, an SQL-invoked method's as §10.6
    /// designates it.
    fn method_of_list(&self, (auth, schema): (&str, &str), d: &RoutineDesignator, of: &BTreeSet<QualifiedName>) -> Result<QualifiedName> {
        let named = qualify(&d.name, schema);
        let specific = if d.specific && of.contains(&named) && self.catalog.routine(&named).is_none() {
            named
        } else {
            self.function_designated(d, (schema, auth), "§12.3 SR6")?.specific
        };
        if !of.contains(&specific) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §12.3 SR6] {specific} is no method of the table's structured type")));
        }
        Ok(specific)
    }

    /// Whether `auth` holds EXECUTE on the method of specific name
    /// `specific`: as the owner of its schema, or as granted on its
    /// SQL-invoked method, observer or mutator.
    fn executes_method(&self, auth: &str, specific: &QualifiedName) -> bool {
        let catalog = &*self.catalog;
        let (who, owner) = (sole(catalog, auth), owner_of(catalog, &specific.auth));
        if let Some(r) = catalog.routine(specific) {
            return r.execute_of(who).0;
        }
        let types = catalog.structured_types();
        let mut implicit =
            types.iter().filter(|t| t.name.auth == specific.auth).flat_map(|t| t.attributes.iter()).flat_map(|a| [&a.observer, &a.mutator]);
        implicit.find(|r| r.specific == specific.ident).is_some_and(|r| usage_held(&r.execute, who, owner).0)
    }

    /// [ISO/IEC 9075-2:2023 §11.57 GR3] Of each table of the type `udt` or
    /// a subtype, each table/method privilege granted on the method
    /// `original` as one on `overriding` instead.
    pub(in crate::db) fn methods_overridden(&mut self, udt: &QualifiedName, (original, overriding): (&QualifiedName, &QualifiedName)) {
        let of_family = |t: &QualifiedName| {
            t == udt
                || self.catalog.structured_type(t).is_some_and(|s| s.supertypes.iter().any(|n| n.schema == udt.auth && n.name == udt.ident))
        };
        let tables =
            self.catalog.tables().into_iter().filter_map(|t| t.typed.as_ref().filter(|d| of_family(&d.udt)).map(|_| t.name.clone()));
        let views = self.catalog.views().into_iter().filter_map(|v| v.typed.as_ref().filter(|d| of_family(&d.udt)).map(|_| v.name.clone()));
        let named: Vec<QualifiedName> = tables.chain(views).collect();
        let mut catalog = self.catalog.duplicate();
        for n in named {
            let mut privileges: Vec<_> = self.catalog.privileges(&n).into_iter().cloned().collect();
            for p in &mut privileges {
                if p.actions.typed.methods.remove(original) {
                    p.actions.typed.methods.insert(overriding.clone());
                }
            }
            catalog.set_privileges(&n, privileges);
        }
        self.catalog = Arc::from(catalog);
    }
}
