//! \[EXTENSION\] Registered XQuery modules (RULES.md D106): CREATE XQUERY
//! MODULE, DROP XQUERY MODULE and what depends on a registered XQuery
//! module. \[XQuery\] leaves where a module import finds the modules of
//! its target namespace to the implementation [XQuery §4.11, §5.2.5]:
//! turborel finds registered ones.
//!
//! A registered XQuery module is a library module [XQuery §4.2], analysed
//! as it is registered, in a static context of its own
//! (`xquery::library_module`): its module imports import among the
//! registered XQuery modules, and its schema imports among the registered
//! XML Schemas, the owner of its schema may use, as a registered XML
//! Schema reads the documents of those its schema's owner may use
//! (`definitions/xml_schemas.rs`). Those it imported are recorded: it
//! depends on them, and imports them again whenever a query imports it.

mod alter;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::revoke::Abandoned;
use super::super::{State, rule};
use super::Defined;
use crate::ast::XQueryModuleDefinition;
use crate::binder::qualify;
use crate::binder::{Registered, in_scope, registered};
use crate::catalog::{Horizon, QualifiedName, Stamps, UsageDesc, XQueryModuleDesc, owner_of, sole};
use crate::error::{Result, SqlState};
use crate::txn::LockMode;
use crate::xquery::{Analysed, Environment, library_module};

impl State {
    /// \[EXTENSION\] CREATE XQUERY MODULE `m`, its name in schema `schema`
    /// if it names none: its text analysed as a library module, its
    /// imports among what its schema's owner may use, and registered,
    /// depending on what it imported.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its schema is not there (SQLSTATE 3F000), a
    /// registered XQuery module of its name is, or its target namespace is
    /// the zero-length string; SQLSTATE 10000 of a text that is no library
    /// module or raises a static error \[XQuery\] names; SQLCODE -900 past
    /// `xquery::MAX_IMPORT_DEPTH`.
    pub(in crate::db) fn create_xquery_module(&mut self, schema: &str, m: &XQueryModuleDefinition) -> Result<Defined> {
        let qn = qualify(&m.name, schema);
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[EXTENSION] there is no schema {0}: CREATE SCHEMA AUTHORIZATION {0} first", qn.auth);
            return Err(rule(msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(m.name.at));
        }
        if self.catalog.xquery_module(&qn).is_some() {
            return Err(rule(format!("[EXTENSION] registered XQuery module {qn} exists: one name identifies one")).or_at(m.name.at));
        }
        let (desc, locks) = self.described(&qn, m, (m.location.clone().unwrap_or_default(), Vec::new()))?;
        let mut catalog = self.catalog.duplicate();
        catalog.put_xquery_module(desc);
        self.catalog = Arc::from(catalog);
        Ok(Defined { locks, ..Defined::default() })
    }

    /// The descriptor of the registered XQuery module `qn` CREATE or ALTER
    /// `m` makes, of location `location` and USAGE grants `usage`: its text
    /// analysed as a library module, its imports among what its schema's
    /// owner may use, and what it imported recorded; and the locks of its
    /// schema and of what it imports.
    ///
    /// # Errors
    ///
    /// As `create_xquery_module`.
    fn described(
        &self,
        qn: &QualifiedName,
        m: &XQueryModuleDefinition,
        (location, usage): (String, Vec<UsageDesc>),
    ) -> Result<(XQueryModuleDesc, Vec<(QualifiedName, LockMode)>)> {
        let held = &*self.catalog;
        let found = registered(held, sole(held, owner_of(held, &qn.auth)), None);
        let analysed = self.analysed(qn, (&m.text, &location), &found)?;
        if analysed.namespace.is_empty() {
            let msg =
                format!("[EXTENSION] [XQuery §4.11] registered XQuery module {qn} has a target namespace, as a module import names one");
            return Err(rule(msg).or_at(m.name.at));
        }
        let modules: Vec<QualifiedName> =
            analysed.modules.iter().filter_map(|n| found.modules.iter().find(|(_, l)| l.name == *n)).map(|(q, _)| q.clone()).collect();
        let xml_schemas: Vec<QualifiedName> =
            analysed.schemas.iter().filter_map(|n| found.importable.iter().find(|(_, s)| s.name == *n)).map(|(q, _)| q.clone()).collect();
        let mut locks = vec![(QualifiedName::of_schema(&qn.auth), LockMode::Shared)];
        locks.extend(modules.iter().chain(&xml_schemas).filter(|u| *u != qn).map(|u| (u.clone(), LockMode::Shared)));
        let desc = XQueryModuleDesc {
            name: qn.clone(),
            target_namespace: analysed.namespace,
            location,
            text: m.text.clone(),
            modules,
            xml_schemas,
            usage,
            // [ISO/IEC 9075-2:2023 §11.60 GR5] as made now; ALTER keeps
            // the creation timestamp it had.
            stamps: Stamps::at(self.stamp),
            module: Some(analysed.module),
        };
        Ok((desc, locks))
    }

    /// The library module `text`, of location `location`, analysed as
    /// `qn`, importing among `found` (`xquery::library_module`).
    ///
    /// # Errors
    ///
    /// As `xquery::library_module`.
    fn analysed(&self, qn: &QualifiedName, (text, location): (&str, &str), found: &Registered) -> Result<Analysed> {
        let env = Environment {
            namespaces: Vec::new(),
            default_element: None,
            variables: Vec::new(),
            library: &self.xquery_functions,
            collations: &self.collations,
            schemas: in_scope(&found.importable).into_iter().map(|(_, s)| s).collect(),
            importable: found.importable.iter().map(|(_, s)| s.clone()).collect(),
            modules: found.modules.iter().map(|(_, l)| l.clone()).collect(),
            linked: found.linked.clone(),
        };
        library_module(text, (&qn.to_string(), &qn.ident), location, &env)
    }

    /// \[EXTENSION\] DROP XQUERY MODULE `qn`: with `cascade`, with what
    /// depends on it (`xquery_module_dependents`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is not registered; if a generated column's query
    /// imports it, even with `cascade`; without `cascade`, if anything
    /// depends on it.
    pub(in crate::db) fn drop_xquery_module(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if self.catalog.xquery_module(qn).is_none() {
            return Err(rule(format!("[EXTENSION] there is no registered XQuery module {qn}")));
        }
        let (mut abandoned, typed) = self.xquery_module_dependents(std::slice::from_ref(qn), &|_| true);
        // Of a cycle, it imports those that import it: it is dropped here.
        abandoned.xquery_modules.retain(|m| m != qn);
        if let Some(t) = typed.first() {
            return Err(rule(format!(
                "[EXTENSION] a generated column of {t} imports registered XQuery module {qn}, so it is not dropped, even with CASCADE"
            )));
        }
        if !cascade && !abandoned.is_empty() {
            let msg =
                format!("[EXTENSION] {} depends on registered XQuery module {qn}, so it is dropped only with CASCADE", abandoned.first());
            return Err(rule(msg));
        }
        let (mut dropped, _) = self.destroy(Arc::clone(&self.catalog), &abandoned)?;
        let mut catalog = self.catalog.duplicate();
        catalog.remove_xquery_module(qn);
        self.catalog = Arc::from(catalog);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// What depends on the registered XQuery modules `gone`, abandoned:
    /// the registered XQuery modules of the schemas `within` takes that
    /// import one, and those that import one of those, of any schema, in
    /// `Abandoned::xquery_modules`, one of `gone` among them if it imports
    /// one of them, through a cycle; and the views, constraints, assertions,
    /// triggers and routines whose queries import one of them, of the
    /// schemas `within` takes for those of `gone`, of any for the others,
    /// as their horizons keep them (`catalog::Horizon::xquery_modules`),
    /// with what invokes such a routine. Apart, the tables of a generated
    /// column whose query imports one, which nothing drops. The one place
    /// what depends on a module is found: for DROP, for a REVOKE of USAGE,
    /// and for what a registered XML Schema dropped takes.
    pub(in crate::db) fn xquery_module_dependents(
        &self,
        gone: &[QualifiedName],
        within: &dyn Fn(&str) -> bool,
    ) -> (Abandoned, Vec<QualifiedName>) {
        let all = self.catalog.xquery_modules();
        let mut modules: Vec<QualifiedName> =
            all.iter().filter(|d| within(&d.name.auth) && d.modules.iter().any(|u| gone.contains(u))).map(|d| d.name.clone()).collect();
        // Each found once, so the walk ends with the catalog's modules, a
        // cycle among them too.
        let mut next = 0_usize;
        while let Some(used) = modules.get(next).cloned() {
            let users = all.iter().filter(|d| d.modules.contains(&used) && !modules.contains(&d.name));
            let found: Vec<QualifiedName> = users.map(|d| d.name.clone()).collect();
            modules.extend(found);
            next = next.saturating_add(1);
        }
        let mut abandoned = self.horizons_naming(&|h: &Horizon| imports_any(h, gone), &|_| false, within);
        abandoned.absorb(self.horizons_naming(&|h: &Horizon| imports_any(h, &modules), &|_| false, &|_| true));
        abandoned.xquery_modules = modules;
        // A generated column is no definition a drop takes away alone: its
        // table keeps it.
        let typed = std::mem::take(&mut abandoned.columns).into_iter().map(|(table, _)| table).collect();
        (abandoned, typed)
    }

    /// [ISO/IEC 9075:1992 §11.2 GR5] The registered XQuery modules of
    /// schema `auth`, which is being dropped, each as DROP XQUERY MODULE
    /// with CASCADE drops it, but those a drop before took.
    ///
    /// # Errors
    ///
    /// As `drop_xquery_module`.
    pub(in crate::db) fn drop_schema_xquery_modules(&mut self, auth: &str) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for qn in self.schema_xquery_modules(auth) {
            if self.catalog.xquery_module(&qn).is_some() {
                dropped.absorb(self.drop_xquery_module(&qn, true)?);
            }
        }
        Ok(dropped)
    }

    /// The names of the registered XML Schemas, then those of the
    /// registered XQuery modules, of schema `auth`.
    pub(in crate::db) fn schema_registered(&self, auth: &str) -> Vec<QualifiedName> {
        let mut names = self.schema_xml_schemas(auth);
        names.extend(self.schema_xquery_modules(auth));
        names
    }

    /// Drops the registered XQuery modules `abandoned` holds, but those a
    /// drop before took, as DROP XQUERY MODULE drops them, adding each to
    /// `dropped` and its lock to `locks`.
    pub(in crate::db) fn drop_abandoned_modules(
        &mut self,
        abandoned: &Abandoned,
        (dropped, locks): (&mut Dropped, &mut Vec<(QualifiedName, LockMode)>),
    ) {
        let mut catalog = self.catalog.duplicate();
        for m in abandoned.xquery_modules.iter().filter(|m| self.catalog.xquery_module(m).is_some()) {
            catalog.remove_xquery_module(m);
            dropped.names.push(m.clone());
            locks.push((m.clone(), LockMode::Exclusive));
        }
        self.catalog = Arc::from(catalog);
    }

    /// The names of the registered XQuery modules of schema `auth`.
    pub(in crate::db) fn schema_xquery_modules(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.xquery_modules().into_iter().filter(|d| d.name.auth == auth).map(|d| d.name.clone()).collect()
    }
}

/// Whether the definition of horizon `horizon` imported one of `modules`.
fn imports_any(horizon: &Horizon, modules: &[QualifiedName]) -> bool {
    horizon.xquery_modules.iter().any(|m| modules.contains(m))
}
