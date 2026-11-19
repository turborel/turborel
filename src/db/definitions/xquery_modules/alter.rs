//! \[EXTENSION\] ALTER XQUERY MODULE (RULES.md D106): a registered XQuery
//! module's text replaced, analysed again and its imports found again
//! and recorded, so a module may import a module registered after it, and
//! modules of one namespace import one another, the cycle XQuery allows
//! [XQuery §4.11].
//!
//! \[DESIGN\] RULES.md D106: what depends on the module, the modules that
//! import it and the definitions that keep it, must still analyse and bind
//! with the new text, as an ALTER of another object is refused that would
//! break what depends on it; so its target namespace may change only if
//! nothing that imports it by that namespace is left.

use std::sync::Arc;

use super::super::super::{State, rule};
use super::super::Defined;
use crate::ast::XQueryModuleDefinition;
use crate::binder::{qualify, registered_module};
use crate::catalog::QualifiedName;
use crate::error::{Result, SqlError};
use crate::xquery::{Environment, Query};

impl State {
    /// \[EXTENSION\] ALTER XQUERY MODULE `m`, its name in schema `schema` if
    /// it names none: its text, and its location if written, replaced, and
    /// what it imports found again among what its schema's owner may use
    /// and recorded, its USAGE grants and creation timestamp kept, its
    /// last-altered timestamp this statement's [ISO/IEC 9075-2:2023 §11.61
    /// GR4].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no registered XQuery module is of its name, its
    /// target namespace is the zero-length string, or a module importing it
    /// or a definition keeping it would no longer analyse or bind; as
    /// CREATE XQUERY MODULE of the text: SQLSTATE 10000 of a static error
    /// \[XQuery\] names, err:XQST0073 of a cycle of imports not all of one
    /// namespace.
    pub(in crate::db) fn alter_xquery_module(&mut self, schema: &str, m: &XQueryModuleDefinition) -> Result<Defined> {
        let qn = qualify(&m.name, schema);
        let Some(old) = self.catalog.xquery_module(&qn).cloned() else {
            return Err(rule(format!("[EXTENSION] there is no registered XQuery module {qn}")).or_at(m.name.at));
        };
        let location = m.location.clone().unwrap_or(old.location);
        let (mut desc, locks) = self.described(&qn, m, (location, old.usage))?;
        desc.stamps.created = old.stamps.created;
        let mut after = self.clone();
        let mut catalog = after.catalog.duplicate();
        catalog.put_xquery_module(desc);
        after.catalog = Arc::from(catalog);
        self.dependents_bind(&after, &qn).map_err(|e| e.or_at(m.name.at))?;
        self.catalog = after.catalog;
        Ok(Defined { locks, ..Defined::default() })
    }

    /// That what depends on registered XQuery module `qn` analyses and
    /// binds in `after`, the definitions as they are with it altered: the
    /// modules that import it, at any remove, and the views, constraints,
    /// assertions, generated columns, triggers and routines that keep it
    /// or one of those.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, naming the first that does not and why.
    fn dependents_bind(&self, after: &State, qn: &QualifiedName) -> Result<()> {
        let (abandoned, typed) = after.xquery_module_dependents(std::slice::from_ref(qn), &|_| true);
        for module in abandoned.xquery_modules.iter().filter(|d| *d != qn) {
            after.module_analyses(module).map_err(|e| broken(qn, &format!("registered XQuery module {module}"), &e))?;
        }
        let unbound = self.unbound_in(after, &|_| true, &|_| true);
        let first = (unbound.views.iter().find(|v| abandoned.views.contains(v)).map(|v| format!("view {v}")))
            .or_else(|| {
                let kept = |t: &QualifiedName, c: &QualifiedName| abandoned.constraints.iter().any(|(u, k)| u == t && k.name == *c);
                unbound.constraints.iter().find(|(t, c)| kept(t, &c.name)).map(|(t, c)| format!("constraint {} of {t}", c.name))
            })
            .or_else(|| {
                unbound
                    .domain_constraints
                    .iter()
                    .find(|d| abandoned.domain_constraints.contains(d))
                    .map(|(d, c)| format!("constraint {c} of domain {d}"))
            })
            .or_else(|| unbound.assertions.iter().find(|a| abandoned.assertions.contains(a)).map(|a| format!("assertion {a}")))
            .or_else(|| unbound.columns.iter().find(|(t, _)| typed.contains(t)).map(|(t, c)| format!("generated column {t}.{c}")));
        if let Some(what) = first {
            return Err(rule(format!("[EXTENSION] {what} would no longer bind with registered XQuery module {qn} altered")));
        }
        for t in abandoned.triggers.iter().filter_map(|t| after.catalog.trigger(t)) {
            after.trigger_binds(t).map_err(|e| broken(qn, &format!("trigger {}", t.name), &e))?;
        }
        for r in abandoned.routines.iter().filter_map(|r| after.catalog.routine(r)) {
            after.routine_binds(r).map_err(|e| broken(qn, &format!("routine {}", r.name), &e))?;
        }
        Ok(())
    }

    /// That the registered XQuery module `name` analyses, with the modules
    /// it imports, as a query importing it analyses it.
    ///
    /// # Errors
    ///
    /// The static error of it, or of a module it imports.
    fn module_analyses(&self, name: &QualifiedName) -> Result<()> {
        let Some(found) = registered_module(&*self.catalog, name) else { return Ok(()) };
        let Some((_, module)) = found.modules.first() else { return Ok(()) };
        let text = format!("import module {} at {}; ()", literal(module.namespace().unwrap_or_default()), literal(&module.name));
        let env = Environment {
            namespaces: Vec::new(),
            default_element: None,
            variables: Vec::new(),
            library: &self.xquery_functions,
            collations: &self.collations,
            schemas: Vec::new(),
            importable: Vec::new(),
            modules: found.modules.iter().map(|(_, m)| m.clone()).collect(),
            linked: found.linked,
            // A library module is typed statically with each query that
            // imports it, as that query's typing asks (RULES.md D106).
            static_typing: false,
            passed: crate::xquery::Passed::default(),
        };
        Query::compile(&text, &env).map(drop)
    }
}

/// `s` as an XQuery string literal [XQuery Appendix A.1 \[144\]]: its
/// quotes doubled, its ampersands written as references.
fn literal(s: &str) -> String {
    format!("\"{}\"", s.replace('&', "&amp;").replace('"', "\"\""))
}

/// The error of `what`, which depends on registered XQuery module `qn`,
/// no longer analysing or binding, `e`, with it altered.
fn broken(qn: &QualifiedName, what: &str, e: &SqlError) -> SqlError {
    rule(format!("[EXTENSION] {what} would no longer analyse or bind with registered XQuery module {qn} altered: {}", e.msg))
}
