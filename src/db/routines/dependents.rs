//! The SQL-invoked routines that depend on what is dropped: on a table or
//! view their bodies name, on a column of one, on a constraint their
//! bodies bind by, on their schema, or on a user-defined type their
//! parameters or result are of [ISO/IEC 9075-2:2023 §11.2, §11.23,
//! §11.26, §11.31, §11.33, §11.59]. Each is dropped with it, as DROP
//! ROUTINE ... CASCADE drops it, or refuses it under RESTRICT. And what
//! invokes a routine dropped [§11.62 SR4c, GR1], read from the subject
//! routines each definition fixed as it was defined [§4.35.2].

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::revoke::Abandoned;
use super::super::{State, rule};
use crate::catalog::{ConstraintDesc, ConstraintKind, Horizon, QualifiedName, SqlRoutineDesc, TableDesc, based_on};
use crate::error::Result;

#[cfg(test)]
mod tests;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.31 SR7e, GR4b iv, §11.33 SR5d, GR3b iv]
    /// The routines whose bodies name one of `gone`, dropped with them if
    /// `cascade`: what was taken away. `table` says whether the first of
    /// `gone` is a base table, for the rule cited.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if a body names one.
    pub(in crate::db) fn drop_routining(&mut self, gone: &[&QualifiedName], (cascade, table): (bool, bool)) -> Result<Dropped> {
        let naming = |r: &SqlRoutineDesc| r.reads.iter().find(|n| gone.contains(n)).cloned();
        let mut doomed = Vec::new();
        for r in self.catalog.routines() {
            let Some(named) = naming(r) else { continue };
            if !cascade {
                let cited = if table { "§11.31 SR7e" } else { "§11.33 SR5d" };
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 {cited}] the body of routine {} references {named}, so it is dropped only with CASCADE",
                    r.specific
                )));
            }
            doomed.push(r.specific.clone());
        }
        self.drop_routines(&doomed)
    }

    /// [ISO/IEC 9075-2:2023 §11.23 SR7c, GR2] The routines whose bodies
    /// bind no more without the column `at` of `desc`, a column dropped,
    /// dropped if `cascade`: what was taken away. The others that see the
    /// table see one column fewer of it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if a body refers to it.
    pub(in crate::db) fn drop_column_routining(&mut self, desc: &TableDesc, at: usize, cascade: bool) -> Result<Dropped> {
        let Some(column) = desc.columns.get(at).map(|c| c.name.clone()) else { return Ok(Dropped::default()) };
        let mut next = desc.clone();
        next.columns.remove(at);
        let without = self.without_column(&next);
        let (mut kept, mut doomed) = (Vec::new(), Vec::new());
        for r in self.catalog.routines().into_iter().filter(|r| r.reads.contains(&desc.name)) {
            let narrowed = SqlRoutineDesc { horizon: r.horizon.without(&desc.name, at), ..r.clone() };
            if without.routine_binds(&narrowed).is_ok() {
                kept.push(narrowed);
            } else if cascade {
                doomed.push(r.specific.clone());
            } else {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.23 SR7c] the body of routine {} refers to {}.{column}, so it is dropped only with CASCADE",
                    r.specific, desc.name
                )));
            }
        }
        let mut catalog = self.catalog.duplicate();
        for r in kept {
            catalog.put_routine(r);
        }
        self.catalog = Arc::from(catalog);
        self.drop_routines(&doomed)
    }

    /// [ISO/IEC 9075-2:2023 §11.26 SR6, SR13d, GR2] The routines naming
    /// `table` whose bodies bind no more once a constraint of it is
    /// dropped, those dependent on it, dropped if `cascade`: what was
    /// taken away. One depends on it if its body bound in the definitions
    /// `before` the drop and binds no more.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if one depends on it.
    pub(in crate::db) fn drop_constraint_routining(&mut self, before: &State, table: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let mut doomed = Vec::new();
        for r in self.catalog.routines().into_iter().filter(|r| r.reads.contains(table)) {
            if self.routine_binds(r).is_ok() || before.routine_binds(r).is_err() {
                continue;
            }
            if !cascade {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.26 SR13d] routine {} depends on the constraint, so it is dropped only with CASCADE",
                    r.specific
                )));
            }
            doomed.push(r.specific.clone());
        }
        self.drop_routines(&doomed)
    }

    /// [ISO/IEC 9075-2:2023 §11.2 SR4l, GR10] The specific names of the
    /// routines of schema `auth`.
    pub(in crate::db) fn schema_routines_of(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.routines().into_iter().filter(|r| r.specific.auth == auth).map(|r| r.specific.clone()).collect()
    }

    /// [ISO/IEC 9075-2:2023 §11.59 SR4g, GR1] The specific names of the
    /// routines whose descriptors include one of the user-defined types
    /// `types`: a parameter or the result of a type based on one. None is
    /// dependent on a type [§4.35.1], as none is made by a type's
    /// definition, so each is dropped with CASCADE, refusing RESTRICT.
    pub(in crate::db) fn routines_typed_by(&self, types: &[QualifiedName]) -> Vec<QualifiedName> {
        let of = |t: &crate::value::DataType| types.iter().any(|qn| based_on(t, qn));
        let bearing = |r: &&SqlRoutineDesc| r.parameters.iter().any(|p| of(&p.data_type)) || r.returns.as_ref().is_some_and(of);
        self.catalog.routines().into_iter().filter(bearing).map(|r| r.specific.clone()).collect()
    }

    /// [ISO/IEC 9075-2:2023 §11.62 SR4c, GR1, §12.7 GR12o] What invokes
    /// one of the routines of specific names `gone`, of what is not among
    /// them: the views, CHECKs, domain constraints, assertions, generated
    /// columns, triggers and routines whose subject routines include one.
    /// Each definition's were fixed as it was defined [§4.35.2] (RULES.md
    /// D73), so they are read from its descriptor, never found again among
    /// those left.
    pub(in crate::db) fn invoking(&self, gone: &[QualifiedName]) -> Abandoned {
        let on = |h: &Horizon| h.subjects().is_some_and(|s| s.iter().any(|r| gone.contains(r)));
        let checks = |c: &ConstraintDesc| matches!(&c.kind, ConstraintKind::Check { horizon, .. } if on(horizon));
        let views = self.catalog.views().into_iter().filter(|v| on(&v.horizon)).map(|v| v.name.clone()).collect();
        let mut abandoned = Abandoned { views, ..Abandoned::default() };
        for t in self.catalog.tables() {
            let constraints = t.constraints.iter().filter(|c| c.domain.is_none() && checks(c));
            abandoned.constraints.extend(constraints.map(|c| (t.name.clone(), c.clone())));
            let generated = t.columns.iter().filter(|c| c.generation().is_some_and(|g| on(&g.horizon)));
            abandoned.columns.extend(generated.map(|c| (t.name.clone(), c.name.clone())));
        }
        for d in self.catalog.domains() {
            abandoned.domain_constraints.extend(d.constraints.iter().filter(|c| checks(c)).map(|c| (d.name.clone(), c.name.clone())));
        }
        abandoned.assertions = self.catalog.assertions().into_iter().filter(|a| checks(a)).map(|a| a.name.clone()).collect();
        abandoned.triggers = self.catalog.triggers().into_iter().filter(|t| on(&t.horizon)).map(|t| t.name.clone()).collect();
        let invoker = |r: &&SqlRoutineDesc| !gone.contains(&r.specific) && on(&r.horizon);
        abandoned.routines = self.catalog.routines().into_iter().filter(invoker).map(|r| r.specific.clone()).collect();
        abandoned
    }

    /// The definitions without the routines of specific names `gone`: the
    /// catalog `destroy` is given.
    pub(in crate::db) fn without_routines(&self, gone: &[QualifiedName]) -> State {
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        for specific in gone {
            catalog.remove_routine(specific);
        }
        after.catalog = Arc::from(catalog);
        after
    }

    /// The routines of specific names `specifics` dropped, as DROP ROUTINE
    /// ... CASCADE drops them, of those still there: what was taken away.
    ///
    /// # Errors
    ///
    /// As `drop_routine`.
    pub(in crate::db) fn drop_routines(&mut self, specifics: &[QualifiedName]) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for specific in specifics {
            if self.catalog.routine(specific).is_some() {
                dropped.absorb(self.drop_routine(specific, true)?);
            }
        }
        Ok(dropped)
    }
}
