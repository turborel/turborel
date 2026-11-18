//! \[EXTENSION\] What maps a schema by XMLMAP (RULES.md D106): the views,
//! SQL-invoked routines and triggers whose text holds `XMLMAP ( SCHEMA s`,
//! which depend on schema `s`, as a constraint a schema names does on it
//! (`drop_named_in`): DROP SCHEMA RESTRICT is refused while one is, and
//! CASCADE drops each. Each is found by reading its text as the lexer
//! does (`parser::mapped_sources`), so a name is compared as its
//! identifier is, in case and quoted.
//! A CHECK constraint and an assertion hold no XMLMAP [ISO/IEC 9075:1992
//! §11.9 SR6], and a mapping of the catalog depends on no schema: a
//! mapping reads the tables there are as it runs.

use super::super::ddl::Dropped;
use super::super::{State, rule};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::parser::{MappedSource, mapped_sources};

/// The views, routines (by specific name) and triggers that map a schema.
#[derive(Debug, Default)]
struct Mappers {
    views: Vec<QualifiedName>,
    routines: Vec<QualifiedName>,
    triggers: Vec<QualifiedName>,
}

/// Whether the SQL text `text` maps schema `schema` by XMLMAP.
fn maps_schema(text: &str, schema: &str) -> bool {
    mapped_sources(text).contains(&MappedSource::Schema(schema.to_owned()))
}

impl State {
    /// The views, routines and triggers of the catalog that map the
    /// schema `auth`, but those of it, which go with it.
    fn mappers_of(&self, auth: &str) -> Mappers {
        let catalog = &*self.catalog;
        let views = catalog.views().into_iter().filter(|v| v.name.auth != auth && maps_schema(&v.query, auth));
        let routines = catalog.routines().into_iter().filter(|r| r.specific.auth != auth && maps_schema(&r.text, auth));
        let triggers = catalog.triggers().into_iter().filter(|t| t.name.auth != auth && maps_schema(&t.text, auth));
        Mappers {
            views: views.map(|v| v.name.clone()).collect(),
            routines: routines.map(|r| r.specific.clone()).collect(),
            triggers: triggers.map(|t| t.name.clone()).collect(),
        }
    }

    /// What maps the schema `auth`, dropped with it, with `cascade` or not:
    /// each with CASCADE.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is there and not `cascade`; as each drop.
    pub(super) fn drop_mappers(&mut self, auth: &str, cascade: bool) -> Result<Dropped> {
        let mappers = &self.mappers_of(auth);
        let first = mappers.views.first().or(mappers.routines.first()).or(mappers.triggers.first());
        if let (Some(first), false) = (first, cascade) {
            return Err(rule(format!("[EXTENSION] {first} maps schema {auth} by XMLMAP, so it is dropped only with CASCADE")));
        }
        let mut dropped = Dropped::default();
        for view in &mappers.views {
            if self.catalog.view(view).is_some() {
                dropped.absorb(self.drop_view(view, true)?);
            }
        }
        dropped.absorb(self.drop_routines(&mappers.routines)?);
        dropped.absorb(self.drop_triggers(&mappers.triggers)?);
        Ok(dropped)
    }
}
