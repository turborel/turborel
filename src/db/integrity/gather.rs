//! \[EXTENSION\] The checks of what a statement did [ISO/IEC 9075:1992
//! §10.6 GR3], gathered at its end from its `Effects`, held as a list that
//! spills (RULES.md D38): each row's constraints, the NO ACTION checks of
//! the foreign keys referencing a row deleted or re-keyed [§11.8 GR2], the
//! rows of each UNIQUE constraint one after another [§11.7 GR2], each CHECK
//! with a subquery of a table written [§11.6 GR3], and each assertion
//! reading one [§11.34 GR2].

use super::{Check, Effect, Effects};
use crate::catalog::{ConstraintDesc, ConstraintKind, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::Result;
use crate::exec::HeldRows;
use crate::storage::{Row, RowId};

impl State {
    /// The checks of what `effects` did, held as it holds them: of each row
    /// inserted, every constraint of its table; of each updated, those on a
    /// column given a value, and its CHECKs; of each deleted or re-keyed,
    /// the foreign keys that reference it; each CHECK with a subquery of a
    /// table written, or reading one; and each assertion reading one. The
    /// rows of each UNIQUE constraint come one after another, read in a
    /// pass of their own over what it did.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn checks(&self, effects: &mut Effects<'_>) -> Result<HeldRows<Check>> {
        let mut out = HeldRows::new(effects.space.clone());
        let mut changed: Vec<QualifiedName> = Vec::new();
        effects.done.visit(&mut |effect| {
            let (table, id, columns) = match effect {
                Effect::Inserted { table, id } => (table, Some(*id), None),
                Effect::Updated { table, id, columns, old } => {
                    self.referenced_checks(table, Some(columns), (old, Some(*id)), &mut out)?;
                    (table, Some(*id), Some(columns.as_slice()))
                }
                Effect::Deleted { table, old, .. } => {
                    self.referenced_checks(table, None, (old, None), &mut out)?;
                    (table, None, None)
                }
            };
            if !changed.contains(table) {
                changed.push(table.clone());
            }
            match id {
                Some(id) => row_checks((self.catalog.table(table), &self.constraints_of(table)), (table, id), columns, &mut out),
                None => Ok(()),
            }
        })?;
        // [§11.6 GR3, §11.34 GR2] a table whose rows it wrote again whole,
        // or took away at once, is one written.
        for table in effects.rewritten_tables() {
            if !changed.contains(table) {
                changed.push(table.clone());
            }
        }
        self.unique_checks(effects, &changed, &mut out)?;
        // [ISO/IEC 9075-2:2023 §4.25.1] one of a supertable checked of each
        // subtable's rows too.
        for desc in self.catalog.tables() {
            for c in self.constraints_of(&desc.name) {
                let ConstraintKind::Check { reads, .. } = &c.kind else { continue };
                if !reads.is_empty() && (changed.contains(&desc.name) || reads.iter().any(|t| changed.contains(t))) {
                    out.push(Check::Table { table: desc.name.clone(), constraint: c.name.clone() })?;
                }
            }
        }
        for a in self.catalog.assertions() {
            let ConstraintKind::Check { reads, .. } = &a.kind else { continue };
            if reads.iter().any(|t| changed.contains(t)) {
                out.push(Check::Assertion { constraint: a.name.clone() })?;
            }
        }
        Ok(out)
    }

    /// Adds to `out` the checks of each UNIQUE constraint of the tables
    /// `changed`, of each row `effects` inserted, or updated giving a value
    /// to one of its columns: one pass over what it did for each, so the
    /// rows of a constraint come one after another.
    ///
    /// # Errors
    ///
    /// The spill area's.
    fn unique_checks(&self, effects: &mut Effects<'_>, changed: &[QualifiedName], out: &mut HeldRows<Check>) -> Result<()> {
        for desc in changed.iter().filter_map(|t| self.catalog.table(t)) {
            for c in self.constraints_of(&desc.name) {
                let ConstraintKind::Unique { columns, period, .. } = &c.kind else { continue };
                // [ISO/IEC 9075-2:2023 §11.7 SR6d i] with a period's columns.
                let of = desc.key_columns(columns, period.as_deref());
                effects.done.visit(&mut |effect| match keyed(effect, desc, &of) {
                    Some(id) => out.push(Check::Unique { table: desc.name.clone(), constraint: c.name.clone(), id }),
                    None => Ok(()),
                })?;
            }
        }
        Ok(())
    }

    /// Adds to `out` the NO ACTION checks of the foreign keys that
    /// reference `table`, a row of which, `old`, was deleted, or (with its
    /// `id`) updated, giving values to `columns`: of those whose referenced
    /// columns it gave values to.
    ///
    /// # Errors
    ///
    /// The spill area's.
    fn referenced_checks(
        &self,
        table: &QualifiedName,
        columns: Option<&[usize]>,
        (old, id): (&Row, Option<RowId>),
        out: &mut HeldRows<Check>,
    ) -> Result<()> {
        // [ISO/IEC 9075-2:2023 §4.17.9] a row of a subtable of the referenced
        // table is a superrow's of it; and a child's foreign keys are its
        // own and its supertables'. The row's id is of the referenced table's
        // row alone.
        for child in self.catalog.tables() {
            for (name, fk) in self.foreign_keys_of(&child.name).into_iter().filter(|(_, fk)| self.reaches(table, &fk.table)) {
                // [ISO/IEC 9075-2:2023 §11.8 SR18a i] of a temporal one,
                // its period's columns too.
                let parent = self.catalog.table(&fk.table);
                let key = parent.map_or_else(|| fk.referenced.clone(), |p| fk.referenced_key(p));
                if columns.is_none_or(|set| key.iter().any(|r| set.contains(r))) {
                    let id = id.filter(|_| fk.table == *table);
                    out.push(Check::Referenced { child: child.name.clone(), constraint: name.clone(), old: old.clone(), id })?;
                }
            }
        }
        Ok(())
    }
}

/// Whether `columns` given values, or every column if none, include one of
/// `of`.
fn assigned(of: &[usize], columns: Option<&[usize]>) -> bool {
    columns.is_none_or(|set| of.iter().any(|x| set.contains(x)))
}

/// Adds to `out` the checks of row `id` of `table`, described by `desc`,
/// written, giving values to `columns`, or to every column if none: of its
/// `constraints` on a column given a value, but UNIQUE ones
/// (`State::checks`), and of its CHECKs without a subquery.
///
/// # Errors
///
/// The spill area's.
fn row_checks(
    (desc, constraints): (Option<&TableDesc>, &[&ConstraintDesc]),
    (table, id): (&QualifiedName, RowId),
    columns: Option<&[usize]>,
    out: &mut HeldRows<Check>,
) -> Result<()> {
    for c in constraints {
        let of_row = match &c.kind {
            ConstraintKind::NotNull(at) => assigned(std::slice::from_ref(at), columns),
            ConstraintKind::ForeignKey(fk) => assigned(&desc.map_or_else(|| fk.columns.clone(), |d| fk.referencing_key(d)), columns),
            ConstraintKind::Check { reads, .. } => reads.is_empty(),
            ConstraintKind::Unique { .. } => false,
        };
        if of_row {
            out.push(Check::Row { table: table.clone(), constraint: c.name.clone(), id })?;
        }
    }
    Ok(())
}

/// The row of `desc` `effect` inserted, or updated giving a value to one of
/// the columns `of`, if it did.
fn keyed(effect: &Effect, desc: &TableDesc, of: &[usize]) -> Option<RowId> {
    let (table, id, columns) = match effect {
        Effect::Inserted { table, id } => (table, *id, None),
        Effect::Updated { table, id, columns, .. } => (table, *id, Some(columns.as_slice())),
        Effect::Deleted { .. } => return None,
    };
    (*table == desc.name && assigned(of, columns)).then_some(id)
}
