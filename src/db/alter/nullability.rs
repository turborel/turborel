//! ALTER COLUMN … SET NOT NULL [ISO/IEC 9075-2:2023 §11.15] and DROP NOT
//! NULL [§11.16], Feature F383 (RULES.md D399, D400).
//!
//! SET NOT NULL gives the column a NOT NULL constraint of a name no
//! constraint of the schema has [§11.15 GR1b], so its descriptor holds the
//! indication that it is NOT NULL and the constraint's name [GR1c], as a
//! NOT NULL written in its definition does. The rows are then checked of
//! it as an ADD CONSTRAINT's are [GR1d] (`db/ddl.rs`), and a null fails
//! with SQLSTATE 23000. A column NOT NULL already is left as it is [GR1].
//!
//! DROP NOT NULL drops each NOT NULL constraint of the column as DROP
//! CONSTRAINT … CASCADE does [§11.16 GR1c], so what depends on it goes
//! with it. A column of a PRIMARY KEY, of a period, or an identity column
//! [§11.12 SR6] keeps it.

use crate::ast::{AlterColumnAction, DropBehavior, TableName};
use crate::catalog::{ColumnDesc, ConstraintDesc, ConstraintKind, Generated, QualifiedName, TableDesc};
use crate::db::constraints::{Described, Within};
use crate::db::ddl::Dropped;
use crate::db::{State, api, rule};
use crate::error::Result;

/// [ISO/IEC 9075-2:2023 §11.12 SR6] That column `c` of `qn` is no identity
/// column, whose actions are its identity's alone.
///
/// # Errors
///
/// SQLCODE -103 if it is one.
pub(super) fn not_identity(qn: &QualifiedName, c: &ColumnDesc) -> Result<()> {
    match c.identity() {
        Some(_) => Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.12 SR6] column {} of {qn} is an identity column, altered by its identity's actions alone",
            c.name
        ))),
        None => Ok(()),
    }
}

/// [ISO/IEC 9075-2:2023 §11.16 SR2, SR3] The period of `desc` whose start
/// or end is the column at `at`, if any, and whether it is the
/// system-time period.
pub(super) fn bounded_period(desc: &TableDesc, at: usize) -> Option<(&str, bool)> {
    let column = desc.columns.get(at)?;
    if let Some(Generated::SystemTime { .. }) = &column.generated {
        return Some(("SYSTEM_TIME", true));
    }
    desc.periods.iter().find(|p| p.bounds(&column.name)).map(|p| (p.name.as_str(), false))
}

impl State {
    /// Column `column` of base table `qn` made NOT NULL, or not, as
    /// `action` says, the constraint it is given named in schema `names`.
    /// What a DROP NOT NULL took away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such column [ISO/IEC 9075:1992
    /// §11.12 SR3], or it is an identity column [ISO/IEC 9075-2:2023 §11.12
    /// SR6]; as `drop_not_null`.
    pub(super) fn alter_nullability(
        &mut self,
        (qn, names): (&QualifiedName, &str),
        column: &str,
        action: &AlterColumnAction,
    ) -> Result<Dropped> {
        let desc = self.base_table(qn)?;
        let (at, c) =
            desc.column_desc(column).ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.12 SR3] {qn} has no column {column}")))?;
        not_identity(qn, c)?;
        match action {
            AlterColumnAction::SetNotNull => self.set_not_null((qn, names), &desc, at).map(|()| Dropped::default()),
            AlterColumnAction::DropNotNull => self.drop_not_null(qn, &desc, at),
            AlterColumnAction::SetDefault(_)
            | AlterColumnAction::DropDefault
            | AlterColumnAction::Identity { .. }
            | AlterColumnAction::DropIdentity
            | AlterColumnAction::DropExpression
            | AlterColumnAction::AddScope(_)
            | AlterColumnAction::DropScope(_)
            | AlterColumnAction::Rename(_)
            | AlterColumnAction::SetDataType(_) => Err(api("[DESIGN] not SET NOT NULL or DROP NOT NULL")),
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.15 GR1] The column at `at` of `desc`, unless
    /// NOT NULL already, given a NOT NULL constraint of a name free in the
    /// schema [GR1b, GR1c]; its rows are checked as the statement ends
    /// [GR1d].
    fn set_not_null(&mut self, (qn, names): (&QualifiedName, &str), desc: &TableDesc, at: usize) -> Result<()> {
        if desc.columns.get(at).is_some_and(|c| c.not_null) {
            return Ok(());
        }
        let mut added = Described::new((qn, names, Within::Session), &desc.columns, self.all_constraint_names());
        added.not_null(at)?;
        let mut next = desc.clone();
        next.constraints.extend(added.constraints);
        next.refresh_not_null();
        self.inherited_nullability(&mut next, &[]);
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(next);
        self.catalog = std::sync::Arc::from(catalog);
        self.renullified(qn);
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.16 GR1] Each NOT NULL constraint of the
    /// column at `at` of `desc` dropped with CASCADE [GR1c]. What went
    /// with them.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the column bounds a period [SR2, SR3] or is in the
    /// PRIMARY KEY [SR4]; as DROP CONSTRAINT.
    fn drop_not_null(&mut self, qn: &QualifiedName, desc: &TableDesc, at: usize) -> Result<Dropped> {
        let column = desc.columns.get(at).map_or("", |c| c.name.as_str());
        if let Some((period, system)) = bounded_period(desc, at) {
            let clause = if system { "SR2" } else { "SR3" };
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.16 {clause}] column {column} of {qn} bounds the period {period}, so it stays NOT NULL"
            )));
        }
        if desc.primary_key().is_some_and(|k| k.contains(&at)) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.16 SR4] column {column} of {qn} is in its PRIMARY KEY, so it stays NOT NULL"
            )));
        }
        // A domain's constraint is the domain's, changed by ALTER DOMAIN.
        let own = |c: &&ConstraintDesc| c.kind == ConstraintKind::NotNull(at) && c.domain.is_none();
        let named: Vec<QualifiedName> = desc.constraints.iter().filter(own).map(|c| c.name.clone()).collect();
        let mut dropped = Dropped::default();
        for name in named {
            let written = TableName { auth: Some(name.auth.clone()), ident: name.ident.clone(), at: 0 };
            dropped.absorb(self.drop_constraint((qn, &name.auth), &written, DropBehavior::Cascade)?);
        }
        Ok(dropped)
    }
}
