//! An application-time period added by ALTER TABLE [ISO/IEC 9075-2:2023
//! §11.27] or dropped [§11.28]: its descriptor, and its period constraint
//! added, the table's rows checked by it, or dropped.

use std::sync::Arc;

use super::{Cause, Made};
use crate::ast::{ColumnDefinition, DropBehavior, PeriodDefinition, PeriodName, SYSTEM_TIME, TableName};
use crate::binder::bind_generation;
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc};
use crate::db::State;
use crate::db::constraints::periods::{Defining, constraint_name, period_check, period_of};
use crate::db::ddl::Dropped;
use crate::db::{api, rule};
use crate::error::{At, Result};
use crate::txn::Txn;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.27] Adds the application-time period `def`
    /// to base table `qn`, its names of schema `names`: its period
    /// constraint, CHECK (start < end), added as ALTER TABLE ADD CONSTRAINT
    /// adds one, so the table's rows are judged by it \[GR2b\], then the
    /// period's descriptor \[GR2c\]. Of `made` and `tx` as `add_constraint`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table is temporary \[SR2\] or typed \[SR3\], or
    /// has an application-time period already \[SR5b\], or a rule of the
    /// period's columns is broken (`periods::period_of`), or columns are
    /// added with it \[SR5d\]; -910 of SYSTEM_TIME (`add_system_period`).
    pub(in crate::db) fn add_period(
        &mut self,
        (qn, names): (&QualifiedName, &str),
        (def, added): (&PeriodDefinition, &[ColumnDefinition]),
        (made, tx): (Option<&Made>, Option<&Txn>),
    ) -> Result<Made> {
        let PeriodName::Application(name) = &def.name else { return Err(api("[DESIGN] SYSTEM_TIME is added by add_system_period")) };
        if let Some(c) = added.first() {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §11.27 SR5d] application-time period {name} adds no column {}", c.name)).or_at(c.at)
            );
        }
        let desc = self.base_table(qn).or_at(def.at)?;
        if desc.temporary.is_some() {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §11.27 SR2] {qn} is a temporary table, not a persistent base table")).or_at(def.at)
            );
        }
        if desc.typed.is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.27 SR3] {qn} is a referenceable table")).or_at(def.at));
        }
        if let Some(p) = desc.periods.iter().find(|p| p.name != SYSTEM_TIME) {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §11.27 SR5b] {qn} has application-time period {} already", p.name)).or_at(def.at)
            );
        }
        let constraint = constraint_name(qn, name, &self.all_constraint_names());
        let period = period_of(&desc, def, Some(constraint.clone()), Defining::Added).or_at(def.at)?;
        let built = self.add_constraint((qn, names), &period_check(def, &constraint)?, (made, tx)).or_at(def.at)?;
        let mut constrained = self.base_table(qn)?;
        constrained.periods.push(period);
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(constrained);
        self.catalog = Arc::from(catalog);
        Ok(built)
    }

    /// [ISO/IEC 9075-2:2023 §11.28] Drops the period `name` of base table
    /// `qn` as `behavior` says: its descriptor \[GR2b\]; what references
    /// it, a view, a table or domain constraint, an assertion, a routine
    /// or a trigger that bound before and binds no more, refusing it under
    /// RESTRICT \[SR3d\] and dropped under CASCADE \[GR2f\]; then its
    /// period constraint, as ALTER TABLE DROP CONSTRAINT CASCADE drops it
    /// \[GR2c\]. What it took away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such period [SR2b, SR3a], or, under
    /// RESTRICT, something references it [SR3d i to vi]; or a generation
    /// expression does, whatever the behavior [SR3d vii] (CONFORMANCE.md,
    /// Readings of ambiguous text).
    pub(in crate::db) fn drop_period(&mut self, qn: &QualifiedName, name: &PeriodName, behavior: DropBehavior) -> Result<Dropped> {
        let table = self.base_table(qn)?;
        let Some(at) = table.periods.iter().position(|p| p.name == name.name()) else {
            let cited = if *name == PeriodName::SystemTime { "SR2b" } else { "SR3a" };
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.28 {cited}] {qn} has no period {}", name.name())));
        };
        let mut dropped = self.drop_period_keys(qn, (&table, at), behavior)?;
        let mut desc = self.base_table(qn)?;
        let before = self.clone();
        let removed = desc.periods.remove(at);
        // [SR3d vii] GR2f drops no generated column, so none may reference
        // it under CASCADE either.
        for (k, c) in desc.columns.iter().enumerate() {
            let Some(g) = c.generation() else { continue };
            if bind_generation(self, &desc, (k, g)).is_err() {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.28 SR3d vii] the generation expression of column {} of {qn} references period {}",
                    c.name, removed.name
                )));
            }
        }
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(desc);
        self.catalog = Arc::from(catalog);
        let cascade = behavior == DropBehavior::Cascade;
        dropped.absorb(self.drop_dependents_of(&before, (Cause::Period, std::slice::from_ref(qn)), cascade)?);
        dropped.absorb(self.drop_dependent_triggers(&before, (Cause::Period, qn), cascade)?);
        dropped.absorb(self.drop_dependent_routines(&before, (Cause::Period, qn), cascade)?);
        if let Some(c) = &removed.constraint {
            let constraint = TableName { auth: Some(c.auth.clone()), ident: c.ident.clone(), at: 0 };
            dropped.absorb(self.drop_constraint((qn, &c.auth), &constraint, DropBehavior::Cascade)?);
        }
        Ok(dropped)
    }

    /// [ISO/IEC 9075-2:2023 §11.28 SR3d ii, GR2f iii] The constraints whose
    /// search conditions reference the period at `at` of `desc`, base table
    /// `qn`: its unique constraints WITHOUT OVERLAPS of it, and the foreign
    /// keys of a period that is it, of `qn` or referencing it; dropped as
    /// ALTER TABLE DROP CONSTRAINT CASCADE drops them under CASCADE.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 under RESTRICT if it has one.
    fn drop_period_keys(&mut self, qn: &QualifiedName, (desc, at): (&TableDesc, usize), behavior: DropBehavior) -> Result<Dropped> {
        let period = desc.periods.get(at).map(|p| p.name.as_str());
        // The temporal foreign keys of either side first, then the keys.
        let mut keyed: Vec<(QualifiedName, QualifiedName)> = Vec::new();
        for table in self.catalog.tables() {
            for c in &table.constraints {
                let ConstraintKind::ForeignKey(fk) = &c.kind else { continue };
                let Some((own, theirs)) = &fk.period else { continue };
                if (table.name == *qn && Some(own.as_str()) == period) || (fk.table == *qn && Some(theirs.as_str()) == period) {
                    keyed.push((table.name.clone(), c.name.clone()));
                }
            }
        }
        for c in &desc.constraints {
            let ConstraintKind::Unique { period: Some(p), .. } = &c.kind else { continue };
            if Some(p.as_str()) == period {
                keyed.push((qn.clone(), c.name.clone()));
            }
        }
        let mut dropped = Dropped::default();
        for (table, name) in keyed {
            // One a CASCADE before it dropped is gone.
            if !self.catalog.table(&table).is_some_and(|t| t.constraints.iter().any(|c| c.name == name)) {
                continue;
            }
            if behavior == DropBehavior::Restrict {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.28 SR3d ii] constraint {name} of {table} is of period {} of {qn}, so it is dropped only with CASCADE",
                    period.unwrap_or_default()
                )));
            }
            let written = TableName { auth: Some(name.auth.clone()), ident: name.ident.clone(), at: 0 };
            dropped.absorb(self.drop_constraint((&table, &name.auth), &written, DropBehavior::Cascade)?);
        }
        Ok(dropped)
    }
}
