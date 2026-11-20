//! System-versioned tables [ISO/IEC 9075-2:2023 §4.17.2.3, §11.3, §11.27
//! to §11.30]: the history of one made, as CREATE TABLE WITH SYSTEM
//! VERSIONING or ALTER TABLE ADD SYSTEM VERSIONING makes the table one,
//! and dropped with DROP SYSTEM VERSIONING or the table; the system-time
//! period added with its two columns, or dropped with them.
//!
//! \[DESIGN\] A table's historical system rows are kept in its history, a
//! table of DEFINITION_SCHEMA of its columns and engine, of no constraint
//! (RULES.md D82): that it has one is what makes the table
//! system-versioned.

use std::sync::Arc;

use super::depending::Cause;
use super::{Made, Reshape};
use crate::ast::{AlterAction, ColumnDefinition, ColumnGeneration, DropBehavior, PeriodDefinition, PeriodName, SYSTEM_TIME, Schema};
use crate::catalog::{Catalog, QualifiedName, TableDesc};
use crate::db::State;
use crate::db::constraints::periods::{Defining, period_of};
use crate::db::ddl::Dropped;
use crate::db::rule;
use crate::db::sequences::Epochs;
use crate::db::write::system_time::history_name;
use crate::error::{At, Result};
use crate::system::DEFINITION_SCHEMA;
use crate::value::DataType;

impl State {
    /// [ISO/IEC 9075-2:2023 §4.17.2.3] Whether `table` is a
    /// system-versioned table: it has a history.
    pub(in crate::db) fn versioned(&self, table: &QualifiedName) -> bool {
        self.catalog.table(&history_name(table)).is_some()
    }

    /// [ISO/IEC 9075-2:2023 §11.3 GR1c, §11.29 GR1] The history of `base`,
    /// made in `catalog`, its storage `made`'s if given: of its columns,
    /// none generated, of its engine and options, a generation after the
    /// last of its name. What was made.
    ///
    /// # Errors
    ///
    /// The storage cannot be made.
    pub(in crate::db) fn make_history(&mut self, catalog: &mut dyn Catalog, base: &TableDesc, made: Option<&Made>) -> Result<Made> {
        let name = history_name(&base.name);
        let generation = self.retired.get(&name).map_or(0, |g| g.saturating_add(1));
        let mut columns = base.columns.clone();
        for c in &mut columns {
            c.generated = None;
            c.default = None;
            c.domain = None;
        }
        let desc = TableDesc {
            name,
            columns,
            constraints: Vec::new(),
            periods: Vec::new(),
            typed: None,
            temporary: None,
            generation,
            engine: base.engine.clone(),
            options: base.options.clone(),
        };
        if catalog.schema_exists(DEFINITION_SCHEMA) {
            catalog.add_table(desc.clone());
        } else {
            catalog.create_schema(DEFINITION_SCHEMA, DEFINITION_SCHEMA, vec![desc.clone()]);
        }
        self.place_table(catalog, &desc, made)
    }

    /// [ISO/IEC 9075-2:2023 §11.30 GR2] The history of `table`, if it has
    /// one, dropped from `catalog`, its storage taken into `dropped`: its
    /// historical system rows deleted.
    pub(in crate::db) fn drop_history(&mut self, catalog: &mut dyn Catalog, table: &QualifiedName, dropped: &mut Dropped) {
        let history = history_name(table);
        if catalog.table(&history).is_none() {
            return;
        }
        catalog.remove_table(&history);
        let generation = self.generation_of(&history);
        self.retired.entry(history.clone()).and_modify(|g| *g = (*g).max(generation)).or_insert(generation);
        dropped.storage.extend(self.storage.remove(&history));
        dropped.names.push(history);
    }

    /// [ISO/IEC 9075-2:2023 §11.3 GR1c, §11.4 SR14] Of CREATE TABLE `desc`
    /// WITH SYSTEM VERSIONING, in `catalog`, its history made, `made`'s if
    /// given. What was made, by the history's name.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a column is of a reference type \[SR14\]; the
    /// storage cannot be made.
    pub(in crate::db) fn versioned_table(
        &mut self,
        catalog: &mut dyn Catalog,
        desc: &TableDesc,
        made: Option<&Made>,
    ) -> Result<(QualifiedName, Made)> {
        if let Some(c) = desc.columns.iter().find(|c| matches!(c.data_type, DataType::Reference(_))) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.4 SR14] column {} of system-versioned table {} is of a reference type",
                c.name, desc.name
            );
            return Err(rule(msg));
        }
        let name = history_name(&desc.name);
        let history = self.make_history(catalog, desc, made.and_then(|m| m.of_table(&name)))?;
        Ok((name, history))
    }

    /// [ISO/IEC 9075-2:2023 §11.1, §11.3 GR1c] Of the tables schema `s`
    /// defines, each WITH SYSTEM VERSIONING given its history, `made`'s if
    /// given, among what `built` holds.
    ///
    /// # Errors
    ///
    /// As `versioned_table`.
    pub(in crate::db) fn schema_histories(&mut self, s: &Schema, made: Option<&Made>, built: &mut Made) -> Result<()> {
        for t in s.tables.iter().filter(|t| t.versioning.is_some()) {
            let qn = QualifiedName { auth: s.name.clone(), ident: t.name.ident.clone() };
            let desc = self.base_table(&qn)?;
            let mut catalog = self.catalog.duplicate();
            let history = self.versioned_table(&mut *catalog, &desc, made).or_at(t.name.at)?;
            self.catalog = Arc::from(catalog);
            built.tables.push(history);
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.29] ALTER TABLE `qn` ADD SYSTEM VERSIONING:
    /// its history made, `made`'s if given.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no regular persistent base table \[SR2\], a
    /// referenceable table \[SR3\], has no system-time period \[SR4\], is
    /// system-versioned \[SR5\], or has a column of a reference type
    /// \[SR6\].
    pub(in crate::db) fn add_versioning(&mut self, qn: &QualifiedName, made: Option<&Made>) -> Result<Made> {
        let desc = self.base_table(qn)?;
        let refused = |sr: &str, what: &str| Err(rule(format!("[ISO/IEC 9075-2:2023 §11.29 {sr}] {qn} {what}")));
        if desc.temporary.is_some() {
            return refused("SR2", "is a temporary table, not a regular persistent base table");
        }
        if desc.typed.is_some() {
            return refused("SR3", "is a referenceable table");
        }
        if desc.period_columns(SYSTEM_TIME).is_none() {
            return refused("SR4", "has no system-time period");
        }
        if self.versioned(qn) {
            return refused("SR5", "is a system-versioned table already");
        }
        if desc.columns.iter().any(|c| matches!(c.data_type, DataType::Reference(_))) {
            return refused("SR6", "has a column of a reference type");
        }
        let mut catalog = self.catalog.duplicate();
        let history = self.make_history(&mut *catalog, &desc, made.and_then(|m| m.of_table(&history_name(qn))))?;
        self.catalog = Arc::from(catalog);
        Ok(Made { tables: vec![(history_name(qn), history)], ..Made::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.30] ALTER TABLE `qn` DROP SYSTEM
    /// VERSIONING `behavior`: its history dropped, its historical system
    /// rows with it \[GR2\]. The views, table and domain constraints,
    /// assertions, routines and triggers that read it FOR SYSTEM_TIME bind
    /// no more, and are dropped under CASCADE \[GR3\]. What it took away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no system-versioned table \[SR2\], or under
    /// RESTRICT if one of those reads it so \[SR3\].
    pub(in crate::db) fn drop_versioning(&mut self, qn: &QualifiedName, behavior: DropBehavior) -> Result<Dropped> {
        self.base_table(qn)?;
        if !self.versioned(qn) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.30 SR2] {qn} is not a system-versioned table")));
        }
        let before = self.clone();
        let mut dropped = Dropped::default();
        let mut catalog = self.catalog.duplicate();
        self.drop_history(&mut *catalog, qn, &mut dropped);
        self.catalog = Arc::from(catalog);
        let cascade = behavior == DropBehavior::Cascade;
        dropped.absorb(self.drop_dependents_of(&before, (Cause::Versioning, std::slice::from_ref(qn)), cascade)?);
        dropped.absorb(self.drop_dependent_triggers(&before, (Cause::Versioning, qn), cascade)?);
        dropped.absorb(self.drop_dependent_routines(&before, (Cause::Versioning, qn), cascade)?);
        Ok(dropped)
    }

    /// [ISO/IEC 9075-2:2023 §11.27 SR4, GR1] ALTER TABLE `qn` ADD PERIOD FOR
    /// SYSTEM_TIME `def`, its columns `added`, the names they write without
    /// a schema of schema `names`, a datetime without a displacement of
    /// `zone`'s: ALTER TABLE ADD COLUMN of each \[GR1a, GR1b\], then the
    /// period's descriptor \[GR1d\]. Its rows take the transaction timestamp
    /// as their start and the highest value as their end as they are
    /// copied (`db/ddl.rs`) \[GR1c\]. What it made, the first column's
    /// storage among its tables, and how its rows change.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule broken.
    pub(in crate::db) fn add_system_period(
        &mut self,
        (qn, names, zone): (&QualifiedName, &str, i16),
        (def, added): (&PeriodDefinition, &[ColumnDefinition]),
        (made, epochs): (Option<&Made>, &mut Epochs),
    ) -> Result<(Dropped, Made, Option<Reshape>)> {
        let desc = self.base_table(qn).or_at(def.at)?;
        system_period_rules(&desc, def, added).or_at(def.at)?;
        let actions: Vec<AlterAction> = added.iter().map(|c| AlterAction::AddColumn(c.clone())).collect();
        let (dropped, built) = self.column_steps((qn, names, zone), &actions, (made, epochs))?;
        let described = self.base_table(qn).and_then(|next| Ok((period_of(&next, def, None, Defining::AddedSystem).or_at(def.at)?, next)));
        let (period, mut next) = match described {
            Ok(described) => described,
            Err(e) => {
                built.discard();
                return Err(e);
            }
        };
        next.periods.push(period);
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(next);
        self.catalog = Arc::from(catalog);
        Ok((dropped, built, Some(Reshape::Two(None))))
    }

    /// Each of `actions`, ALTER TABLE `qn` ADD or DROP COLUMN, in turn, the
    /// first of the storage `made` keeps among its tables, the last of
    /// `made`'s own; what each made but the last discarded if one fails,
    /// else kept among the tables of what the last made, whose rows are
    /// copied once. What they took away, and what they made.
    ///
    /// # Errors
    ///
    /// An action's.
    fn column_steps(
        &mut self,
        (qn, names, zone): (&QualifiedName, &str, i16),
        actions: &[AlterAction],
        (made, epochs): (Option<&Made>, &mut Epochs),
    ) -> Result<(Dropped, Made)> {
        let mut dropped = Dropped::default();
        let mut firsts: Vec<Made> = Vec::new();
        for (k, action) in actions.iter().enumerate() {
            let step = if k.saturating_add(1) < actions.len() { made.and_then(|m| m.of_table(qn)) } else { made };
            match self.alter_columns((qn, names, zone), action, (step, epochs)) {
                Ok((gone, built, _)) => {
                    dropped.absorb(gone);
                    firsts.push(built);
                }
                Err(e) => {
                    firsts.iter().for_each(Made::discard);
                    return Err(e);
                }
            }
        }
        let mut built = firsts.pop().unwrap_or_default();
        built.tables.extend(firsts.into_iter().map(|first| (qn.clone(), first)));
        Ok((dropped, built))
    }

    /// [ISO/IEC 9075-2:2023 §11.28 GR1] The system-time period of `qn`, its
    /// descriptor dropped (`drop_period`), dropped with its start and end
    /// columns, as ALTER TABLE DROP COLUMN drops each \[GR1a, GR1b\], as
    /// `behavior` says \[SR2c\]. What it took away, what it made, and how its
    /// rows change.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is a system-versioned table \[SR2a\], or a column
    /// may not be dropped so.
    pub(in crate::db) fn drop_system_period(
        &mut self,
        qn: &QualifiedName,
        behavior: DropBehavior,
        made: Option<&Made>,
    ) -> Result<(Dropped, Made, Option<Reshape>)> {
        if self.versioned(qn) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.28 SR2a] {qn} is a system-versioned table, so its system-time period is kept"
            )));
        }
        let desc = self.base_table(qn)?;
        let Some([start, end]) = desc.period_columns(SYSTEM_TIME) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.28 SR2b] {qn} has no period SYSTEM_TIME")));
        };
        let names: Vec<String> = [start, end].iter().filter_map(|&at| desc.columns.get(at).map(|c| c.name.clone())).collect();
        let mut dropped = self.drop_period(qn, &PeriodName::SystemTime, behavior)?;
        // The end's position once the start is gone.
        let positions = [start, if end > start { end.saturating_sub(1) } else { end }];
        let actions: Vec<AlterAction> = names.into_iter().map(|column| AlterAction::DropColumn { column, at: 0, behavior }).collect();
        let (gone, built) = self.column_steps((qn, "", 0), &actions, (made, &mut Epochs::fresh()))?;
        dropped.absorb(gone);
        Ok((dropped, built, Some(Reshape::Two(Some(positions)))))
    }
}

/// [ISO/IEC 9075-2:2023 §11.11 SR2, §11.12 SR2, §11.23 SR3] That `action`
/// of ALTER TABLE `qn`, of `st`, adds, alters or drops no column of a
/// system-versioned table.
///
/// # Errors
///
/// SQLCODE -103 if it does.
pub(super) fn unversioned(st: &State, qn: &QualifiedName, action: &AlterAction) -> Result<()> {
    let (rule_of, at) = match action {
        AlterAction::AddColumn(def) => {
            if let Some(ColumnGeneration::SystemTime { at, .. }) = def.generated.as_deref() {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §11.4 SR6a] column {} of ROW START or ROW END is added by ADD PERIOD FOR SYSTEM_TIME",
                    def.name
                );
                return Err(rule(msg).or_at(*at));
            }
            ("§11.11 SR2", def.at)
        }
        AlterAction::AlterColumn { at, .. } => ("§11.12 SR2", *at),
        AlterAction::DropColumn { at, .. } => ("§11.23 SR3", *at),
        AlterAction::AddConstraint(_)
        | AlterAction::DropConstraint { .. }
        | AlterAction::AddPeriod(..)
        | AlterAction::DropPeriod { .. }
        | AlterAction::AddVersioning(_)
        | AlterAction::DropVersioning { .. }
        | AlterAction::RevertCollation(_)
        | AlterAction::Truncate { .. }
        | AlterAction::AttachPartition { .. }
        | AlterAction::DetachPartition(_)
        | AlterAction::RenameTable { .. }
        | AlterAction::Storage(_) => return Ok(()),
    };
    if st.versioned(qn) {
        let msg =
            format!("[ISO/IEC 9075-2:2023 {rule_of}] {qn} is a system-versioned table, whose columns are not added, altered or dropped");
        return Err(rule(msg).or_at(at));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.27 SR4] Of ALTER TABLE `desc` ADD PERIOD FOR
/// SYSTEM_TIME `def`, its columns `added`: the table has no system-time
/// period \[SR4a\] and no column SYSTEM_TIME \[SR4b\]; the columns are
/// given \[SR4c\], one of ROW START, the period's start, then one of ROW
/// END, its end \[SR4d to SR4f\]; and it is a
/// persistent base table \[SR2\], not a referenceable one \[SR3\].
///
/// # Errors
///
/// SQLCODE -103 for the rule broken.
fn system_period_rules(desc: &TableDesc, def: &PeriodDefinition, added: &[ColumnDefinition]) -> Result<()> {
    let qn = &desc.name;
    let refused = |sr: &str, what: String| Err(rule(format!("[ISO/IEC 9075-2:2023 §11.27 {sr}] {what}")));
    if desc.temporary.is_some() {
        return refused("SR2", format!("{qn} is a temporary table, not a persistent base table"));
    }
    if desc.typed.is_some() {
        return refused("SR3", format!("{qn} is a referenceable table"));
    }
    if desc.period_columns(SYSTEM_TIME).is_some() {
        return refused("SR4a", format!("{qn} has a system-time period already"));
    }
    if desc.column(SYSTEM_TIME).is_some() {
        return refused("SR4b", format!("{qn} has a column named SYSTEM_TIME"));
    }
    let [first, second] = added else {
        return refused("SR4c, SR4d", "ADD PERIOD FOR SYSTEM_TIME adds two columns, of ROW START and ROW END".to_owned());
    };
    let row =
        |c: &ColumnDefinition| if let Some(ColumnGeneration::SystemTime { end, .. }) = c.generated.as_deref() { Some(*end) } else { None };
    if row(first) != Some(false) || row(second) != Some(true) {
        return refused("SR4d", "ADD PERIOD FOR SYSTEM_TIME adds a column of ROW START, then one of ROW END".to_owned());
    }
    if first.name != def.start.0 || second.name != def.end.0 {
        let msg = format!("the period's columns are {} and {}, those added {} and {}", def.start.0, def.end.0, first.name, second.name);
        return refused("SR4e, SR4f", msg);
    }
    // [SR4g] Of one type: `period_of` judges the columns added.
    Ok(())
}

#[cfg(test)]
mod tests;
