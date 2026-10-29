//! \[EXTENSION\] A table's descriptor with a column added [ISO/IEC
//! 9075:1992 §11.11] or dropped [§11.15], with its constraints. Split from
//! db/alter.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::Reshape;
use crate::ast::{ColumnDefinition, DropBehavior};
use crate::catalog::{ColumnDesc, ConstraintDesc, ConstraintKind, QualifiedName, TableDesc};
use crate::db::State;
use crate::db::constraints::Described;
use crate::db::defaults::default_of;
use crate::error::{Code, Result, SqlError};
use crate::logical::cond_columns;
use crate::settings::Kind;

/// A Syntax Rule violation.
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}

impl State {
    /// [ISO/IEC 9075:1992 §11.11] `desc` with the column `def` added last,
    /// its names of schema `names`, with its constraints [GR4], of the next
    /// generation.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule it breaks: its name is a column's [§11.4
    /// SR1], or a constraint's; -104 for a foreign key it may not make.
    pub(in crate::db) fn add_column(&self, (desc, names, zone): (&TableDesc, &str, i16), def: &ColumnDefinition) -> Result<TableDesc> {
        if desc.column(&def.name).is_some() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.4 SR1] {} already has a column {}", desc.name, def.name)));
        }
        let mut next = desc.clone();
        // [ISO/IEC 9075:1992 §11.4 SR5, AR1] its data type, or its domain's.
        let (data_type, domain) = self.column_type(names, &def.data_type)?;
        let default = def.default.as_ref().map(|o| default_of(data_type, o, zone)).transpose()?;
        next.columns.push(ColumnDesc { name: def.name.clone(), data_type, not_null: false, default, domain });
        let taken = self.constraint_names(&desc.name.auth);
        let mut added = Described::new((&desc.name, names), &next.columns, taken);
        added.reserve(def.constraints.iter().filter_map(|c| c.name.as_ref()));
        added.column(desc.columns.len(), def)?;
        let (constraints, pending) = (added.constraints, added.pending);
        next.constraints.extend(constraints);
        // [§11.21 GR5, GR6] its domain's constraints, on it.
        let column = next.columns.last().map(std::slice::from_ref).unwrap_or_default();
        let copies = self.domain_checks(&desc.name, column);
        next.constraints.extend(copies);
        for p in &pending {
            let resolved = self.resolve_foreign_key(names, &next, p)?;
            next.constraints.push(resolved);
        }
        Described::check_unique(&next)?;
        next.refresh_not_null();
        next.generation = desc.generation.saturating_add(1);
        Ok(next)
    }

    /// [ISO/IEC 9075:1992 §11.15] `desc` without the column `column`, of
    /// the next generation, the other columns renumbered [GR6]; and the
    /// other tables whose foreign keys reference it, those keys renumbered.
    /// A constraint of the column alone goes with it; one that refers to
    /// other columns too, or a foreign key to it, only if `behavior` is
    /// CASCADE [SR4, GR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the column is not one of the table, or its only one
    /// [SR3], or a constraint refers to it and other columns and
    /// `behavior` is RESTRICT [SR4].
    pub(in crate::db) fn drop_column(
        &self,
        desc: &TableDesc,
        column: &str,
        behavior: DropBehavior,
    ) -> Result<(TableDesc, Reshape, Vec<TableDesc>)> {
        let at = desc.column(column).ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.15 SR3] {} has no column {column}", desc.name)))?;
        self.unnamed_by_options(desc, column)?;
        if desc.columns.len() == 1 {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.15 SR3] {column} is the only column of {}", desc.name)));
        }
        let reshape = Reshape::Remove(at);
        let refused = |c: &ConstraintDesc| {
            rule(format!(
                "[ISO/IEC 9075:1992 §11.15 SR4] constraint {c} refers to {}.{column} and other columns, so it is dropped only with CASCADE",
                desc.name
            ))
        };
        let mut next = desc.clone();
        next.columns.remove(at);
        next.constraints.clear();
        let without = self.without_column(&next);
        for c in &desc.constraints {
            let (refers, alone) = self.refers_to(&without, desc, c, at)?;
            if !refers {
                next.constraints.push(renumbered(c, desc, reshape, true));
            } else if !alone && behavior == DropBehavior::Restrict {
                return Err(refused(c));
            }
        }
        let mut others = Vec::new();
        for other in self.catalog.tables().into_iter().filter(|t| t.name != desc.name) {
            let mut changed = other.clone();
            changed.constraints.clear();
            for c in &other.constraints {
                let refers = refers_from(&without, (desc, at), (&other.name, c));
                if refers && behavior == DropBehavior::Restrict {
                    return Err(refused(c));
                }
                if !refers {
                    changed.constraints.push(renumbered(c, desc, reshape, false));
                }
            }
            if changed.constraints != other.constraints {
                changed.refresh_not_null();
                others.push(changed);
            }
        }
        next.refresh_not_null();
        next.generation = desc.generation.saturating_add(1);
        Ok((next, reshape, others))
    }

    /// Refuses `column` of `desc` if an option of columns names it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one does.
    fn unnamed_by_options(&self, desc: &TableDesc, column: &str) -> Result<()> {
        let declared = self.options_of(&desc.engine);
        let naming = desc.options.iter().find(|(name, value)| {
            declared.iter().any(|o| o.name == *name && matches!(o.kind, Kind::Columns { .. }))
                && super::storage::columns_named(value).iter().any(|c| c == column)
        });
        match naming {
            Some((name, _)) => Err(rule(format!(
                "[EXTENSION] the table option {name} of {} names {column}: it is dropped once the option names it no more",
                desc.name
            ))),
            None => Ok(()),
        }
    }

    /// The definitions with `next`, a table with a column dropped, in place
    /// of the table: a CHECK that reads the table refers to the column if
    /// it binds there no more.
    pub(in crate::db) fn without_column(&self, next: &TableDesc) -> State {
        let mut without = self.clone();
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(next.clone());
        without.catalog = Arc::from(catalog);
        without
    }

    /// Whether constraint `c` of `table` refers to its column `at`, and
    /// whether it refers to no other column [§11.15 SR4]: a CHECK with a
    /// subquery, which reads other columns, if it binds no more in
    /// `without`.
    fn refers_to(&self, without: &State, table: &TableDesc, c: &ConstraintDesc, at: usize) -> Result<(bool, bool)> {
        if let ConstraintKind::Check { text, names, reads, horizon } = &c.kind
            && !reads.is_empty()
        {
            return Ok((without.check_condition(&table.name, (text, names, &horizon.without(&table.name, at))).is_err(), false));
        }
        let (uses, alone) = self.columns_of(table, c)?;
        Ok((uses.contains(&at), alone))
    }

    /// The columns of `table` constraint `c` refers to, and whether it is
    /// a constraint of them alone [§11.15 SR4]: a foreign key refers to the
    /// columns it references too.
    fn columns_of(&self, table: &TableDesc, c: &ConstraintDesc) -> Result<(Vec<usize>, bool)> {
        Ok(match &c.kind {
            ConstraintKind::NotNull(at) => (vec![*at], true),
            ConstraintKind::Unique { columns, .. } => (columns.clone(), columns.len() == 1),
            ConstraintKind::ForeignKey(fk) => {
                let mut uses = fk.columns.clone();
                if fk.table == table.name {
                    uses.extend(fk.referenced.iter().copied());
                }
                (uses, false)
            }
            ConstraintKind::Check { text, names, horizon, .. } => {
                let mut uses = Vec::new();
                cond_columns(&self.check_condition(&table.name, (text, names, horizon))?, &mut uses);
                uses.sort_unstable();
                uses.dedup();
                let alone = uses.len() == 1;
                (uses, alone)
            }
        })
    }
}

/// Whether constraint `c` of table `other` refers to the column at `at` of
/// `table` [§11.15 SR4]: a foreign key to it, or a CHECK that reads the
/// table and binds no more in `without`, seeing one column fewer of it.
fn refers_from(without: &State, (table, at): (&TableDesc, usize), (other, c): (&QualifiedName, &ConstraintDesc)) -> bool {
    match &c.kind {
        ConstraintKind::ForeignKey(fk) => fk.table == table.name && fk.referenced.contains(&at),
        ConstraintKind::Check { text, names, reads, horizon } => {
            reads.contains(&table.name) && without.check_condition(other, (text, names, &horizon.without(&table.name, at))).is_err()
        }
        ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } => false,
    }
}

/// `c`, a constraint kept, its columns of `table` renumbered as `reshape`
/// says: its own if it is `own`, one of `table`'s, and those it references
/// in `table`. A CHECK names its columns, so keeps its text.
fn renumbered(c: &ConstraintDesc, table: &TableDesc, reshape: Reshape, own: bool) -> ConstraintDesc {
    let moved = |columns: &[usize]| reshape.columns(columns).unwrap_or_else(|| columns.to_vec());
    let kind = match &c.kind {
        ConstraintKind::NotNull(at) => ConstraintKind::NotNull(if own { reshape.moved(*at).unwrap_or(*at) } else { *at }),
        ConstraintKind::Unique { columns, primary } => {
            ConstraintKind::Unique { columns: if own { moved(columns) } else { columns.clone() }, primary: *primary }
        }
        ConstraintKind::ForeignKey(fk) => {
            let mut fk = fk.clone();
            if own {
                fk.columns = moved(&fk.columns);
            }
            if fk.table == table.name {
                fk.referenced = moved(&fk.referenced);
            }
            ConstraintKind::ForeignKey(fk)
        }
        // Its horizon one column fewer of the table, if it saw the one removed.
        ConstraintKind::Check { text, names, reads, horizon } => {
            let seen = match reshape {
                Reshape::Remove(at) => horizon.without(&table.name, at),
                Reshape::Keep | Reshape::Append => horizon.clone(),
            };
            ConstraintKind::Check { text: text.clone(), names: names.clone(), reads: reads.clone(), horizon: seen }
        }
    };
    ConstraintDesc { name: c.name.clone(), kind, attributes: c.attributes, domain: c.domain.clone() }
}
