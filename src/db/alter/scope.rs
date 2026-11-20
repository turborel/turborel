//! [ISO/IEC 9075-2:2023 §11.17, §11.18] The scope of a reference column
//! added or dropped: ALTER TABLE … ALTER COLUMN … ADD SCOPE and DROP
//! SCOPE.
//!
//! The column's type changes, and so does that of the column of each
//! proper subtable of the table that corresponds to it [§11.17 GR2, §11.18
//! GR6]. No row changes. A scope dropped takes away what dereferences the
//! column: each routine, view, constraint, assertion and trigger that
//! bound before and binds no more, refused under RESTRICT [§11.18 SR5] and
//! dropped under CASCADE [GR1 to GR5] (`db/alter/depending.rs`).

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::depending::Cause;
use super::rule;
use crate::ast::{AlterColumnAction, DropBehavior};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, TableDesc, proper_subtables, udt_qualified};
use crate::db::State;
use crate::error::Result;
use crate::value::DataType;
use crate::value::types::UdtName;

impl State {
    /// [§11.17, §11.18] The scope of column `column`, written at `at`, of
    /// the base table `qn`, its names of schema `names`, added or dropped
    /// as `action` says: what was taken away, and the tables whose
    /// columns changed.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks, or, under RESTRICT, a
    /// dereference of the column a definition contains [§11.18 SR5].
    pub(in crate::db) fn alter_scope(
        &mut self,
        (qn, names): (&QualifiedName, &str),
        column: &str,
        action: &AlterColumnAction,
    ) -> Result<Dropped> {
        let desc = self.base_table(qn)?;
        let adding = matches!(action, AlterColumnAction::AddScope(_));
        let clause = if adding { "§11.17" } else { "§11.18" };
        let c = desc.columns.iter().find(|c| c.name == column);
        let c = c.ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.12 SR3] {qn} has no column {column}")))?;
        let DataType::Reference(r) = &c.data_type else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 {clause} SR1] column {column} of {qn} is of {}, no reference type",
                c.data_type
            )));
        };
        self.originally_defined(&desc, column, clause)?;
        let scope = match action {
            AlterColumnAction::AddScope(table) => {
                Some(self.added_scope(&qualify(table, names), r.scope.is_some(), &udt_qualified(&r.referenced.name))?)
            }
            AlterColumnAction::DropScope(_) if r.scope.is_none() => {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.18 SR1] column {column} of {qn} has no scope to drop")));
            }
            AlterColumnAction::DropScope(_)
            | AlterColumnAction::SetDefault(_)
            | AlterColumnAction::DropDefault
            | AlterColumnAction::Identity { .. }
            | AlterColumnAction::DropIdentity
            | AlterColumnAction::DropExpression
            | AlterColumnAction::Rename(_)
            | AlterColumnAction::SetNotNull
            | AlterColumnAction::DropNotNull
            | AlterColumnAction::SetDataType(_) => None,
        };
        let changed = DataType::reference((*r.referenced).clone(), scope);
        let old = r.scope.as_ref().map(|s| QualifiedName { auth: s.schema.clone(), ident: s.name.clone() });
        let before = self.clone();
        let family = self.rescoped(qn, column, &changed);
        let mut dropped = Dropped { names: family.clone(), ..Dropped::default() };
        if let AlterColumnAction::DropScope(behavior) = action {
            let mut read = family;
            if let Some(s) = old {
                read.extend(proper_subtables(&*self.catalog, &s));
                read.push(s);
            }
            dropped.absorb(self.drop_dereferencing(&before, &read, *behavior == DropBehavior::Cascade)?);
        }
        Ok(dropped)
    }

    /// [§11.17 SR2, §11.18 SR2] That `column` of `desc`, if a referenceable
    /// table, is one of its originally-defined columns: none it inherits
    /// from its direct supertable.
    fn originally_defined(&self, desc: &TableDesc, column: &str, clause: &str) -> Result<()> {
        let Some(sup) = desc.typed.as_ref().and_then(|t| t.supertable.as_ref()) else { return Ok(()) };
        if self.catalog.table(sup).is_some_and(|s| s.column(column).is_some()) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 {clause} SR2] column {column} of {} is inherited from {sup}, no originally-defined column",
                desc.name
            )));
        }
        Ok(())
    }

    /// [§11.17 SR3, SR5] The scope `stn` added to a column of a reference
    /// type of the referenced type `udt`, which has one already if
    /// `scoped`: a referenceable table of that type.
    fn added_scope(&self, stn: &QualifiedName, scoped: bool, udt: &QualifiedName) -> Result<UdtName> {
        if scoped {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.17 SR3] the column has a scope already".to_owned()));
        }
        let typed = self.catalog.table(stn).and_then(|t| t.typed.as_ref());
        if typed.is_none_or(|t| t.udt != *udt) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.17 SR5] {stn} is no referenceable table of type {udt}")));
        }
        Ok(UdtName::new(&stn.auth, &stn.ident))
    }

    /// [§11.17 GR1, GR2, §11.18 GR6] Column `column` of table `qn`, and the
    /// column of each proper subtable that corresponds to it, made of type
    /// `changed`: the tables changed.
    fn rescoped(&mut self, qn: &QualifiedName, column: &str, changed: &DataType) -> Vec<QualifiedName> {
        let family: Vec<QualifiedName> = std::iter::once(qn.clone()).chain(proper_subtables(&*self.catalog, qn)).collect();
        let mut catalog = self.catalog.duplicate();
        for t in &family {
            let Some(mut desc) = self.catalog.table(t).cloned() else { continue };
            desc.columns.iter_mut().filter(|c| c.name == column).for_each(|c| c.data_type = changed.clone());
            catalog.replace_table(desc);
        }
        self.catalog = Arc::from(catalog);
        family
    }

    /// [§11.18 SR5, GR1 to GR5] What contains a dereference of the column
    /// whose scope was dropped, the definitions `before` it was, `read`
    /// the tables it may read: the routines and triggers, and what
    /// `drop_dependents_of` finds, that bound before and bind no more,
    /// dropped if `cascade`.
    fn drop_dereferencing(&mut self, before: &State, read: &[QualifiedName], cascade: bool) -> Result<Dropped> {
        let routines: Vec<QualifiedName> = (self.catalog.routines().into_iter())
            .filter(|r| before.routine_binds(r).is_ok() && self.routine_binds(r).is_err())
            .map(|r| r.specific.clone())
            .collect();
        let triggers: Vec<QualifiedName> = (self.catalog.triggers().into_iter())
            .filter(|t| before.trigger_binds(t).is_ok() && self.trigger_binds(t).is_err())
            .map(|t| t.name.clone())
            .collect();
        if !cascade {
            let what = routines.first().map(|r| ("SR5a", format!("the body of routine {r}")));
            let what = what.or_else(|| triggers.first().map(|t| ("SR5d", format!("the triggered action of trigger {t}"))));
            if let Some((cited, what)) = what {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.18 {cited}] {what} contains a dereference of the column, so its scope is dropped only with CASCADE"
                )));
            }
        }
        let mut dropped = self.drop_routines(&routines)?;
        dropped.absorb(self.drop_triggers(&triggers)?);
        dropped.absorb(self.drop_dependents_of(before, (Cause::Scope, read), cascade)?);
        Ok(dropped)
    }
}
