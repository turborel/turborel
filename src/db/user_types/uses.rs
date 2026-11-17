//! What references a distinct type, found by binding again each view's
//! query, each search condition and each generation expression in the
//! definitions as a statement leaves them: without the type, as DROP TYPE
//! leaves them [ISO/IEC 9075-2:2023 §11.59 SR4f, SR4h], or with the
//! privileges a REVOKE leaves [§12.7 GR12]. One that no longer binds
//! referenced it, by a CAST to it, an array or row type naming it, or a
//! call of its routines; and each column of a type based on it is lost
//! with it [§11.59 SR4b, §12.7 GR12h ii].

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::generated::Column;
use super::super::revoke::Abandoned;
use super::super::{State, api, rule};
use crate::binder::{bind_generation, view_degree};
use crate::catalog::{ConstraintKind, QualifiedName, based_on};
use crate::error::{Result, SqlError};

impl State {
    /// [ISO/IEC 9075-2:2023 §11.59] Drops distinct type `qn`, with each
    /// type based on it, and its routines [GR8, GR9]: with CASCADE, the
    /// views, constraints and assertions that reference them, as revoking
    /// every privilege on them takes them [GR7], and the routines that
    /// reference them [SR4g], the columns of a type based on them and the
    /// generated columns that reference them gone already
    /// (`Session::dropping_generations`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such type \[SR3\], or, not `cascade`,
    /// anything references it [SR4]; -910 if, with `cascade`, a column of
    /// it is still there.
    pub(in crate::db) fn drop_user_type(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        match self.catalog.structured_type(qn).cloned() {
            // [SR4c to SR4e, SR4f iv] What a structured type has more.
            Some(d) if !cascade => self.structured_unreferenced(&d)?,
            Some(_) => {}
            None => {
                self.user_type_named(qn, "§11.59 SR3")?;
                // [SR4e, SR4f iv] Its transform groups and casts.
                if !cascade {
                    self.no_transform_or_cast(qn)?;
                }
            }
        }
        let gone = self.types_based_on(qn);
        // [SR4g] The routines of them, or whose bodies reference them.
        let routines = self.routines_lost_with(&gone);
        if let (Some(r), false) = (routines.first(), cascade) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.59 SR4g] routine {r} references type {qn}, so it is dropped only with CASCADE"
            )));
        }
        // [SR4d] With CASCADE, the typed tables of them.
        let mut dropped = self.drop_typed_tables(&gone)?;
        dropped.absorb(self.drop_routines(&routines)?);
        let after = self.without_types(&gone);
        let abandoned = self.unbound_in(&after, &|_| true, &|_| true);
        let columns = self.columns_based_on(&gone, &|_| true);
        if !cascade {
            unreferenced(qn, &gone, &columns, &abandoned)?;
        }
        if let Some((t, c)) = columns.first() {
            return Err(api(format!("[DESIGN] column {t}.{c} is of a type based on {qn}, and was not dropped first")));
        }
        let (destroyed, locks) = self.destroy(Arc::clone(&after.catalog), &abandoned)?;
        dropped.absorb(destroyed);
        dropped.names.extend(locks.into_iter().map(|(name, _)| name));
        dropped.names.extend(gone);
        Ok(dropped)
    }

    /// [ISO/IEC 9075-2:2023 §11.59 SR4g, GR1, §11.62 GR1] The specific
    /// names of the SQL-invoked routines DROP TYPE with CASCADE of the
    /// types `gone` drops: those of a parameter or result of a type based
    /// on one, those whose bodies bind no more without them, and each that
    /// invokes one of those, at any remove.
    pub(in crate::db) fn routines_lost_with(&self, gone: &[QualifiedName]) -> Vec<QualifiedName> {
        if !self.catalog.any_routines() {
            return Vec::new();
        }
        let mut lost = self.routines_typed_by(gone);
        let after = self.without_types(gone);
        let unbound = self.catalog.routines().into_iter().filter(|r| !lost.contains(&r.specific) && after.routine_binds(r).is_err());
        lost.extend(unbound.map(|r| r.specific.clone()).collect::<Vec<_>>());
        self.invokers_of(&lost)
    }

    /// The columns DROP TYPE `qn` with CASCADE loses, or of each type of
    /// a schema it drops, in the tables of a schema `within` takes: of a
    /// type based on them, and generated columns that reference them.
    pub(in crate::db) fn columns_lost_with(&self, types: &[QualifiedName], within: &dyn Fn(&str) -> bool) -> Vec<Column> {
        let mut gone: Vec<QualifiedName> = Vec::new();
        for qn in types {
            gone.extend(self.types_based_on(qn).into_iter().filter(|t| !gone.contains(t)).collect::<Vec<_>>());
        }
        let mut columns = self.columns_based_on(&gone, within);
        // Those that invoke a routine lost with them.
        let routined = self.invoking(&self.routines_lost_with(&gone)).columns.into_iter().filter(|(t, _)| within(&t.auth));
        columns.extend(routined.filter(|c| !columns.contains(c)).collect::<Vec<_>>());
        let after = self.without_types(&gone);
        columns.extend(self.unbound_in(&after, &|_| true, within).columns.into_iter().filter(|c| !columns.contains(c)).collect::<Vec<_>>());
        columns
    }

    /// [ISO/IEC 9075-2:2023 §4.2] `qn` and each user-defined type based on
    /// it: a distinct type whose source type is, a structured type that is
    /// a subtype of it or has an attribute that is, and those based on
    /// them in turn, each pass taking one more type at least.
    fn types_based_on(&self, qn: &QualifiedName) -> Vec<QualifiedName> {
        let catalog = &*self.catalog;
        let mut gone = vec![qn.clone()];
        let distinct = catalog.user_types();
        let structured = catalog.structured_types();
        for _ in 0..=distinct.len().saturating_add(structured.len()) {
            let on = |t: &crate::value::DataType| gone.iter().any(|g| based_on(t, g));
            let mut next: Vec<QualifiedName> =
                distinct.iter().filter(|t| !gone.contains(&t.name) && on(&t.source)).map(|t| t.name.clone()).collect();
            let of = |t: &&&crate::catalog::StructuredDesc| {
                !gone.contains(&t.name) && (gone.iter().any(|g| t.is_proper_subtype_of(g)) || t.attributes.iter().any(|a| on(&a.data_type)))
            };
            next.extend(structured.iter().filter(of).map(|t| t.name.clone()));
            if next.is_empty() {
                break;
            }
            gone.extend(next);
        }
        gone.retain(|t| catalog.user_type(t).is_some() || catalog.structured_type(t).is_some());
        gone
    }

    /// The definitions without the types `gone`, their orderings,
    /// transforms and the casts from and to them [§11.59 GR2 to GR6].
    fn without_types(&self, gone: &[QualifiedName]) -> State {
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        let casts: Vec<(crate::value::DataType, crate::value::DataType)> = (self.catalog.casts().into_iter())
            .filter(|c| {
                gone.iter()
                    .any(|g| super::super::structured::names_type(&c.source, g) || super::super::structured::names_type(&c.target, g))
            })
            .map(|c| (c.source.clone(), c.target.clone()))
            .collect();
        for (source, target) in &casts {
            catalog.remove_cast(source, target);
        }
        for qn in gone {
            catalog.remove_user_type(qn);
            catalog.remove_structured_type(qn);
            catalog.remove_ordering(qn);
            catalog.put_transforms(qn, Vec::new());
        }
        after.catalog = Arc::from(catalog);
        after
    }

    /// The columns of the tables of a schema `within` takes whose types
    /// are based on any of `types`.
    fn columns_based_on(&self, types: &[QualifiedName], within: &dyn Fn(&str) -> bool) -> Vec<Column> {
        let mut columns = Vec::new();
        // A typed table of a type lost goes whole, none of its columns alone.
        let whole = |t: &crate::catalog::TableDesc| t.typed.as_ref().is_some_and(|d| types.contains(&d.udt));
        for t in self.catalog.tables().into_iter().filter(|t| within(&t.name.auth) && !whole(t)) {
            let of = t.columns.iter().filter(|c| types.iter().any(|qn| crate::db::structured::names_type(&c.data_type, qn)));
            columns.extend(of.map(|c| (t.name.clone(), c.name.clone())));
        }
        columns
    }

    /// What binds in these definitions but not in `after`, as `lost`
    /// takes its refusal: views, CHECKs, domain constraints and
    /// assertions abandoned, a copy of a domain's constraint on a column
    /// with the domain's [ISO/IEC 9075:1992 §11.37 SR13-SR16]; and the
    /// generated columns of the tables of a schema `within` takes, lost.
    pub(in crate::db) fn unbound_in(&self, after: &State, lost: &dyn Fn(&SqlError) -> bool, within: &dyn Fn(&str) -> bool) -> Abandoned {
        let gone = |r: Result<()>| r.is_err_and(|e| lost(&e));
        let views = self.catalog.views().into_iter().filter(|v| gone(view_degree(after, v).map(|_| ()))).map(|v| v.name.clone()).collect();
        let mut abandoned = Abandoned { views, ..Abandoned::default() };
        for t in self.catalog.tables() {
            for c in t.constraints.iter().filter(|c| c.domain.is_none()) {
                let ConstraintKind::Check { text, names, horizon, .. } = &c.kind else { continue };
                if gone(after.check_condition(&t.name, (text, names, horizon)).map(|_| ())) {
                    abandoned.constraints.push((t.name.clone(), c.clone()));
                }
            }
            for (at, c) in t.columns.iter().enumerate().filter(|_| within(&t.name.auth)) {
                if c.generation().is_some_and(|g| gone(bind_generation(after, t, (at, g)).map(|_| ()))) {
                    abandoned.columns.push((t.name.clone(), c.name.clone()));
                }
            }
        }
        for d in self.catalog.domains() {
            for c in &d.constraints {
                let ConstraintKind::Check { text, names, horizon, .. } = &c.kind else { continue };
                if gone(after.bound_on_value(&d.name, d.data_type.clone(), (text, names, horizon)).map(|_| ())) {
                    abandoned.domain_constraints.push((d.name.clone(), c.name.clone()));
                }
            }
        }
        abandoned.assertions =
            self.catalog.assertions().into_iter().filter(|a| gone(after.assertion_plan(a).map(|_| ()))).map(|a| a.name.clone()).collect();
        abandoned
    }
}

/// [ISO/IEC 9075-2:2023 §11.59 SR4] That nothing references distinct
/// type `qn`, with RESTRICT: no column is of a type based on it, nor on a
/// type of `gone`, those based on it \[SR4b\]; no other type is based on
/// it [SR4f v]; and no view, constraint or generated column, in
/// `abandoned`, references it or calls its routines [SR4f, SR4h].
fn unreferenced(qn: &QualifiedName, gone: &[QualifiedName], columns: &[Column], abandoned: &Abandoned) -> Result<()> {
    let cascade = "so it is dropped only with CASCADE";
    if let Some((t, c)) = columns.first() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4b] column {t}.{c} is of a type based on {qn}, {cascade}")));
    }
    if let Some(other) = gone.iter().find(|t| *t != qn) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4f] type {other} is based on {qn}, {cascade}")));
    }
    if !abandoned.is_empty() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.59 SR4f, SR4h] {} references type {qn}, {cascade}", abandoned.first())));
    }
    Ok(())
}
