//! An application-time period added by ALTER TABLE [ISO/IEC 9075-2:2023
//! §11.27] or dropped [§11.28]: its descriptor, and its period constraint
//! added, the table's rows checked by it, or dropped.

use std::sync::Arc;

use super::Made;
use crate::ast::{DropBehavior, PeriodDefinition, PeriodName, TableName};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::db::constraints::periods::{Defining, application, constraint_name, period_check, period_of};
use crate::db::ddl::Dropped;
use crate::db::rule;
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
    /// period's columns is broken (`periods::period_of`); -900 of
    /// SYSTEM_TIME.
    pub(in crate::db) fn add_period(
        &mut self,
        (qn, names): (&QualifiedName, &str),
        def: &PeriodDefinition,
        (made, tx): (Option<&Made>, Option<&Txn>),
    ) -> Result<Made> {
        let name = application(&def.name).or_at(def.at)?;
        let desc = self.base_table(qn).or_at(def.at)?;
        if desc.temporary.is_some() {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §11.27 SR2] {qn} is a temporary table, not a persistent base table")).or_at(def.at)
            );
        }
        if desc.typed.is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.27 SR3] {qn} is a referenceable table")).or_at(def.at));
        }
        if let Some(p) = desc.periods.first() {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §11.27 SR5b] {qn} has application-time period {} already", p.name)).or_at(def.at)
            );
        }
        let constraint = constraint_name(qn, name, &self.all_constraint_names());
        let period = period_of(&desc, def, constraint.clone(), Defining::Added).or_at(def.at)?;
        let built = self.add_constraint((qn, names), &period_check(def, &constraint)?, (made, tx)).or_at(def.at)?;
        let mut constrained = self.base_table(qn)?;
        constrained.periods.push(period);
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(constrained);
        self.catalog = Arc::from(catalog);
        Ok(built)
    }

    /// [ISO/IEC 9075-2:2023 §11.28] Drops the period `name` of base table
    /// `qn`: its descriptor \[GR2b\], then its
    /// period constraint, as ALTER TABLE DROP CONSTRAINT CASCADE drops it
    /// \[GR2c\]. Nothing yet names a period but its descriptor, so RESTRICT
    /// and CASCADE drop the same \[SR3b to SR3d, GR2d to GR2f\]. What it
    /// took away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such period [SR2b, SR3a].
    pub(in crate::db) fn drop_period(&mut self, qn: &QualifiedName, name: &PeriodName) -> Result<Dropped> {
        let mut desc = self.base_table(qn)?;
        let Some(at) = desc.periods.iter().position(|p| *name == PeriodName::Application(p.name.clone())) else {
            let cited = if *name == PeriodName::SystemTime { "SR2b" } else { "SR3a" };
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.28 {cited}] {qn} has no period {}", name.name())));
        };
        let removed = desc.periods.remove(at);
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(desc);
        self.catalog = Arc::from(catalog);
        let constraint = TableName { auth: Some(removed.constraint.auth.clone()), ident: removed.constraint.ident.clone(), at: 0 };
        self.drop_constraint((qn, &removed.constraint.auth), &constraint, DropBehavior::Cascade)
    }
}
