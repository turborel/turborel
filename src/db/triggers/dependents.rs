//! The triggers that depend on what is dropped: on their subject table,
//! on a table or view their triggered action names, on a column of their
//! trigger column list or triggered action column set, or on a constraint
//! their action binds by [ISO/IEC 9075-2:2023 §11.23, §11.26, §11.31,
//! §11.33]. Each is dropped with it, as DROP TRIGGER drops it, or, but for
//! one on the subject table alone, refuses it under RESTRICT.

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::{State, rule};
use crate::catalog::{QualifiedName, TableDesc, TriggerDesc, TriggerEvent};
use crate::error::Result;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.31 SR7g, GR4b v, §11.33 SR5c, GR3b v] The
    /// triggers whose triggered action names one of `gone`, dropped with
    /// them if `cascade`, and those whose subject table is one of them:
    /// what was taken away. `table` says whether the first of `gone` is a
    /// base table, for the rule cited.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if an action names one.
    pub(in crate::db) fn drop_triggering(&mut self, gone: &[&QualifiedName], (cascade, table): (bool, bool)) -> Result<Dropped> {
        let naming = |t: &TriggerDesc| t.reads.iter().find(|r| gone.contains(r)).cloned();
        let mut doomed = Vec::new();
        for t in self.catalog.triggers() {
            if let (Some(named), false) = (naming(t), cascade) {
                let cited = if table { "§11.31 SR7g" } else { "§11.33 SR5c" };
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 {cited}] the triggered action of trigger {} references {named}, so it is dropped only with CASCADE",
                    t.name
                )));
            }
            if naming(t).is_some() || gone.contains(&&t.table) {
                doomed.push(t.name.clone());
            }
        }
        self.drop_triggers(&doomed)
    }

    /// [ISO/IEC 9075-2:2023 §11.23 SR7e, GR1] The triggers that refer to
    /// the column `at` of `desc`, a column dropped, by their trigger column
    /// list or triggered action column set, or whose action binds no more
    /// without it, dropped if `cascade`: what was taken away. The others
    /// that see the table see one column fewer of it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if one refers to it.
    pub(in crate::db) fn drop_column_triggering(&mut self, desc: &TableDesc, at: usize, cascade: bool) -> Result<Dropped> {
        let Some(column) = desc.columns.get(at).map(|c| c.name.clone()) else { return Ok(Dropped::default()) };
        let mut next = desc.clone();
        next.columns.remove(at);
        let without = self.without_column(&next);
        let (mut kept, mut doomed) = (Vec::new(), Vec::new());
        for t in self.catalog.triggers() {
            if t.table != desc.name && !t.reads.contains(&desc.name) {
                continue;
            }
            let narrowed = TriggerDesc { horizon: t.horizon.without(&desc.name, at), ..t.clone() };
            if !refers(t, &desc.name, &column) && without.trigger_binds(&narrowed).is_ok() {
                kept.push(narrowed);
            } else if cascade {
                doomed.push(t.name.clone());
            } else {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.23 SR7e] trigger {} refers to {}.{column}, so it is dropped only with CASCADE",
                    t.name, desc.name
                )));
            }
        }
        let mut catalog = self.catalog.duplicate();
        for t in kept {
            catalog.put_trigger(t);
        }
        self.catalog = Arc::from(catalog);
        self.drop_triggers(&doomed)
    }

    /// [ISO/IEC 9075-2:2023 §11.26 SR13f, GR5] The triggers on or naming
    /// `table` whose action binds no more once a constraint of it is
    /// dropped, those that depend on it, dropped if `cascade`: what was
    /// taken away. One depends on it if its action bound in the
    /// definitions `before` the drop and binds no more.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if one depends on it.
    pub(in crate::db) fn drop_constraint_triggering(&mut self, before: &State, table: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let mut doomed = Vec::new();
        for t in self.catalog.triggers() {
            if (t.table != *table && !t.reads.contains(table)) || self.trigger_binds(t).is_ok() || before.trigger_binds(t).is_err() {
                continue;
            }
            if !cascade {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §11.26 SR13f] trigger {} depends on the constraint, so it is dropped only with CASCADE",
                    t.name
                );
                return Err(rule(msg));
            }
            doomed.push(t.name.clone());
        }
        self.drop_triggers(&doomed)
    }

    /// [ISO/IEC 9075-2:2023 §11.2 SR4j, GR9] The triggers of schema `auth`.
    pub(in crate::db) fn schema_triggers_of(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.triggers().into_iter().filter(|t| t.name.auth == auth).map(|t| t.name.clone()).collect()
    }

    /// The triggers `names` dropped, as DROP TRIGGER drops them, of those
    /// still there: what was taken away.
    ///
    /// # Errors
    ///
    /// As `drop_trigger`.
    pub(in crate::db) fn drop_triggers(&mut self, names: &[QualifiedName]) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for name in names {
            if self.catalog.trigger(name).is_some() {
                dropped.absorb(self.drop_trigger(name)?);
            }
        }
        Ok(dropped)
    }
}

/// [ISO/IEC 9075-2:2023 §11.23 SR7e] Whether trigger `t` refers to column
/// `column` of `table` by its explicit trigger column list or its
/// triggered action column set.
fn refers(t: &TriggerDesc, table: &QualifiedName, column: &str) -> bool {
    let listed = t.table == *table && matches!(&t.event, TriggerEvent::Update(list) if list.iter().any(|c| c == column));
    listed || t.columns.iter().any(|(of, c)| of == table && c == column)
}
