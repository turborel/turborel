//! \[EXTENSION\] The checks of what a statement did [ISO/IEC 9075:1992
//! §10.6 GR3], gathered at its end from its `Effects`, held as a list that
//! spills (RULES.md D38): each row's constraints, the NO ACTION checks of
//! the foreign keys referencing a row deleted or re-keyed [§11.8 GR2], the
//! rows of each UNIQUE constraint one after another [§11.7 GR2], each CHECK
//! with a subquery of a table written [§11.6 GR3], and each assertion
//! reading one [§11.34 GR2]. Split from integrity.rs (RULES.md R-SIZE-2).

use super::{Check, Effect, Effects};
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc};
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
            match (id, self.catalog.table(table)) {
                (Some(id), Some(desc)) => row_checks(desc, id, columns, &mut out),
                (None, _) | (_, None) => Ok(()),
            }
        })?;
        self.unique_checks(effects, &changed, &mut out)?;
        for desc in self.catalog.tables() {
            for c in &desc.constraints {
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
            for c in &desc.constraints {
                let ConstraintKind::Unique { columns: of, .. } = &c.kind else { continue };
                effects.done.visit(&mut |effect| match keyed(effect, desc, of) {
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
        for child in self.catalog.tables() {
            for (name, fk) in child.foreign_keys().filter(|(_, fk)| fk.table == *table) {
                if columns.is_none_or(|set| fk.referenced.iter().any(|r| set.contains(r))) {
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

/// Adds to `out` the checks of row `id` of `desc` written, giving values
/// to `columns`, or to every column if none: of the constraints of its
/// table on a column given a value, but UNIQUE ones (`State::checks`), and
/// of its CHECKs without a subquery.
///
/// # Errors
///
/// The spill area's.
fn row_checks(desc: &TableDesc, id: RowId, columns: Option<&[usize]>, out: &mut HeldRows<Check>) -> Result<()> {
    for c in &desc.constraints {
        let of_row = match &c.kind {
            ConstraintKind::NotNull(at) => assigned(std::slice::from_ref(at), columns),
            ConstraintKind::ForeignKey(fk) => assigned(&fk.columns, columns),
            ConstraintKind::Check { reads, .. } => reads.is_empty(),
            ConstraintKind::Unique { .. } => false,
        };
        if of_row {
            out.push(Check::Row { table: desc.name.clone(), constraint: c.name.clone(), id })?;
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
