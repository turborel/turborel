//! What depends on a registered XML Schema [ISO/IEC 9075-14:2006 draft
//! §4.2.6], for `State::xml_schema_dependents`: the tables, domains and
//! user-defined types of a column, data type, source or attribute of an
//! XML type of XMLSCHEMA of it [§4.2.2, §6.1 GR1d], which nothing drops;
//! and the definitions that name it, which DROP ... CASCADE drops: the
//! routines of a parameter or result of such a type, and the views,
//! constraints, assertions, triggers and routines whose `<XML valid
//! according to what>`s indicated it as they were defined [§4.2.6], as
//! their horizons keep it (`catalog::Horizon::xml_schemas`), with what
//! invokes such a routine.

use super::super::super::State;
use super::super::super::revoke::Abandoned;
use crate::catalog::xml_schemas::xml_schemas_of;
use crate::catalog::{ConstraintDesc, ConstraintKind, Horizon, QualifiedName};
use crate::value::DataType;

/// Whether `t` is, or holds, an XML type of XMLSCHEMA of one of the
/// registered XML Schemas `gone` (`catalog::xml_schemas::xml_schemas_of`).
fn names_any(t: &DataType, gone: &[QualifiedName]) -> bool {
    xml_schemas_of(t).iter().any(|n| gone.contains(n))
}

/// `more` added to `into`, each not there already.
fn add(into: &mut Vec<QualifiedName>, more: Vec<QualifiedName>) {
    for name in more {
        if !into.contains(&name) {
            into.push(name);
        }
    }
}

impl State {
    /// The tables, domains and user-defined types of the schemas `within`
    /// takes that hold an XML type of XMLSCHEMA of one of `gone`: a
    /// column's type, a domain's data type, a distinct type's source, a
    /// structured type's attribute's type.
    pub(super) fn typed_by_xml_schemas(&self, gone: &[QualifiedName], within: &dyn Fn(&str) -> bool) -> Vec<QualifiedName> {
        let catalog = &*self.catalog;
        let of = |t: &DataType| names_any(t, gone);
        let tables = catalog.tables().into_iter().filter(|t| t.columns.iter().any(|c| of(&c.data_type))).map(|t| t.name.clone());
        let domains = catalog.domains().into_iter().filter(|d| of(&d.data_type)).map(|d| d.name.clone());
        let distinct = catalog.user_types().into_iter().filter(|d| of(&d.source)).map(|d| d.name.clone());
        let structured =
            catalog.structured_types().into_iter().filter(|s| s.attributes.iter().any(|a| of(&a.data_type))).map(|s| s.name.clone());
        tables.chain(domains).chain(distinct).chain(structured).filter(|n| within(&n.auth)).collect()
    }

    /// The definitions of the schemas `within` takes that name one of
    /// `gone`, as DROP ... CASCADE takes them: the routines of a parameter
    /// or result of an XML type of XMLSCHEMA of one, and the views,
    /// constraints, assertions, triggers and routines that indicated one
    /// as they were defined; each routine that invokes such a routine, at
    /// any remove, and what else invokes one [ISO/IEC 9075-2:2023 §11.62
    /// GR1]. Generated columns among them are in `columns`.
    pub(super) fn naming_xml_schemas(&self, gone: &[QualifiedName], within: &dyn Fn(&str) -> bool) -> Abandoned {
        let on = |h: &Horizon| h.xml_schemas.iter().any(|s| gone.contains(s));
        let of = |t: &DataType| names_any(t, gone);
        let routines: Vec<QualifiedName> = (self.catalog.routines().into_iter())
            .filter(|r| within(&r.specific.auth))
            .filter(|r| on(&r.horizon) || r.parameters.iter().any(|p| of(&p.data_type)) || r.returns.as_ref().is_some_and(of))
            .map(|r| r.specific.clone())
            .collect();
        let routines = self.invokers_of(&routines);
        let mut abandoned = if routines.is_empty() { Abandoned::default() } else { self.invoking(&routines) };
        abandoned.routines = routines;
        self.indicating(&mut abandoned, &on, within);
        abandoned
    }

    /// Adds to `abandoned` the views, constraints, assertions, triggers
    /// and generated columns of the schemas `within` takes whose horizons
    /// `on` takes, each once.
    fn indicating(&self, abandoned: &mut Abandoned, on: &dyn Fn(&Horizon) -> bool, within: &dyn Fn(&str) -> bool) {
        let checks = |c: &ConstraintDesc| if let ConstraintKind::Check { horizon, .. } = &c.kind { on(horizon) } else { false };
        let catalog = &*self.catalog;
        let views = catalog.views().into_iter().filter(|v| within(&v.name.auth) && on(&v.horizon)).map(|v| v.name.clone());
        add(&mut abandoned.views, views.collect());
        for t in catalog.tables().into_iter().filter(|t| within(&t.name.auth)) {
            for c in t.constraints.iter().filter(|c| c.domain.is_none() && checks(c)) {
                if !abandoned.constraints.iter().any(|(table, k)| *table == t.name && k.name == c.name) {
                    abandoned.constraints.push((t.name.clone(), c.clone()));
                }
            }
            let generated = t.columns.iter().filter(|c| c.generation().is_some_and(|g| on(&g.horizon)));
            abandoned.columns.extend(generated.map(|c| (t.name.clone(), c.name.clone())));
        }
        for d in catalog.domains().into_iter().filter(|d| within(&d.name.auth)) {
            for c in d.constraints.iter().filter(|c| checks(c)) {
                let named = (d.name.clone(), c.name.clone());
                if !abandoned.domain_constraints.contains(&named) {
                    abandoned.domain_constraints.push(named);
                }
            }
        }
        let assertions = catalog.assertions().into_iter().filter(|a| within(&a.name.auth) && checks(a)).map(|a| a.name.clone());
        add(&mut abandoned.assertions, assertions.collect());
        let triggers = catalog.triggers().into_iter().filter(|t| within(&t.name.auth) && on(&t.horizon)).map(|t| t.name.clone());
        add(&mut abandoned.triggers, triggers.collect());
    }
}
