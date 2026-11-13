//! Identity columns and generated columns [ISO/IEC 9075-2:2023 §4.17.12,
//! §4.17.13, §11.4]: a column's description from its definition, the
//! types and rules of generation expressions, the epochs of identity
//! columns' sequence generators, and the generated columns a column's
//! drop would leave without one of their parametric columns [§11.23].
//! How each row written gets its values is `db/generated/rows.rs`; the
//! generated columns that name a domain or collation that goes are
//! `db/generated/named.rs`.
//!
//! RULES.md D63: an identity column's sequence generator is the table's
//! own, named as the table is, its counter shared and logged as any
//! generator's (D62). A generated column's values are stored, given each
//! row as it is written: its generation expression is kept as written and
//! bound once for each statement that writes the table.

mod named;
mod rows;

pub(super) use named::{Column, Named};
pub(super) use rows::{Generating, check_override};

use super::State;
use super::sequences::{Epochs, generator};
use crate::ast::{ColumnDefinition, ColumnGeneration, References, ReferentialAction};
use crate::binder::{assignable, bind_generation};
use crate::catalog::{ColumnDesc, Generated, Generation, Identity, QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::expr_columns;
use crate::value::DataType;

/// The type a generated column whose definition writes none has until its
/// generation expression is bound [ISO/IEC 9075-2:2023 §11.4 SR13c]: no
/// generation expression references it \[SR10c\], so none reads it.
const UNTYPED: DataType = DataType::Boolean;

/// A Syntax Rule violation.
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}

impl State {
    /// [ISO/IEC 9075:1992 §11.4; ISO/IEC 9075-2:2023 §11.4] The description
    /// of column `def` of table `table`, its names of schema `names`, a
    /// datetime it writes without a displacement of `zone`'s: of a
    /// generated column whose definition writes no type, `UNTYPED` until
    /// `typed_generations` binds it; of an identity column, its generator's
    /// epoch still to give (`give_epochs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule it breaks; -104 if its owner holds no USAGE
    /// on its domain.
    pub(super) fn column_of(&self, (table, names, zone): (&QualifiedName, &str, i16), def: &ColumnDefinition) -> Result<ColumnDesc> {
        // [ISO/IEC 9075:1992 §11.4 SR5, AR1] its data type, or its domain's;
        // [ISO/IEC 9075-2:2023 §11.4 SR11] or none, of a generated column.
        let untyped = || {
            rule(format!(
                "[ISO/IEC 9075-2:2023 §11.4 SR11] column {} has no <generation clause>, so it is defined with a data type or a domain",
                def.name
            ))
        };
        let (data_type, domain) = match (&def.data_type, def.generated.as_deref()) {
            (Some(t), _) => self.column_type(names, t)?,
            (None, Some(ColumnGeneration::Expression { .. })) => (UNTYPED, None),
            (None, Some(ColumnGeneration::Identity { at, .. })) => return Err(untyped().or_at(*at)),
            (None, None) => return Err(untyped().or_at(def.at)),
        };
        let default = def.default.as_ref().map(|o| super::defaults::default_of(&data_type.clone(), o, zone)).transpose()?;
        let collation = match &def.data_type {
            Some(_) => self.declared_collation(names, (data_type.clone(), domain.as_ref()), def.collate.as_ref())?,
            None => None,
        };
        let generated = def.generated.as_ref().map(|g| generated_of(table, (names, data_type.clone()), g)).transpose()?;
        Ok(ColumnDesc { name: def.name.clone(), data_type, not_null: false, default, domain, collation, generated })
    }

    /// [ISO/IEC 9075-2:2023 §11.4 SR10, SR13] The generated columns of
    /// `desc`, defined by `defs`, each with its position, bound over its row: of
    /// one whose definition writes no type, its type that of its generation
    /// expression \[SR13c\], with its `<collate clause>`'s collation; of one
    /// that writes one, its expression assignable to it \[SR13a, SR13b\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule one breaks.
    pub(super) fn typed_generations<'d>(
        &self,
        desc: &mut TableDesc,
        defs: impl Iterator<Item = (usize, &'d ColumnDefinition)>,
    ) -> Result<()> {
        let names = desc.columns.iter().map(|c| c.generation().map(|g| g.names.clone())).collect::<Vec<_>>();
        for (at, def) in defs {
            let Some(Some(schema)) = names.get(at) else { continue };
            let (_, t) = generation_type(self, desc, at).or_at(def.at)?;
            let Some(column) = desc.columns.get_mut(at) else { continue };
            if def.data_type.is_none() {
                column.data_type = t.clone();
                column.collation = self.declared_collation(schema, (t, None), def.collate.as_ref()).or_at(def.at)?;
            } else if !assignable(&column.data_type, &t) {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §11.4 SR13a, SR13b] the generation expression of column {} is of type {t}, not assignable to its type {}",
                    column.name, column.data_type
                );
                return Err(rule(msg).or_at(def.at));
            }
        }
        Ok(())
    }

    /// `columns`, of table `name`, defined by `defs` in order, with
    /// `typed_generations`' types, and at most one an identity column
    /// (`one_identity`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule they break.
    pub(super) fn typed_columns(
        &self,
        name: &QualifiedName,
        columns: Vec<ColumnDesc>,
        defs: &[&ColumnDefinition],
    ) -> Result<Vec<ColumnDesc>> {
        let (constraints, options) = (Vec::new(), Vec::new());
        let mut probe =
            TableDesc { name: name.clone(), columns, constraints, engine: String::new(), options, generation: 0, temporary: None };
        self.typed_generations(&mut probe, defs.iter().copied().enumerate())?;
        Self::one_identity(&probe)?;
        Ok(probe.columns)
    }

    /// [ISO/IEC 9075-2:2023 §11.3 SR15, §11.11 SR3] That table `desc` has at
    /// most one identity column.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it has more.
    pub(super) fn one_identity(desc: &TableDesc) -> Result<()> {
        if desc.columns.iter().filter(|c| c.identity().is_some()).count() > 1 {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR15, §11.11 SR3] {} has at most one identity column", desc.name)));
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.4 GR4] The sequence generator of each
    /// identity column of `desc` given its epoch from `epochs`, as a
    /// definition's (RULES.md D62).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a record logged gives fewer epochs than it took.
    pub(super) fn give_epochs(&self, desc: &mut TableDesc, epochs: &mut Epochs) -> Result<()> {
        for column in &mut desc.columns {
            if let Some(Generated::Identity(identity)) = &mut column.generated
                && identity.sequence.epoch == 0
            {
                identity.sequence.epoch = epochs.next(&self.sequences)?;
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.23 SR7f, GR3] The generated columns of
    /// `desc` other than the one at `at` whose generation expressions
    /// reference it.
    ///
    /// # Errors
    ///
    /// One binds no more.
    pub(super) fn generated_on(&self, desc: &TableDesc, at: usize) -> Result<Vec<String>> {
        let mut on = Vec::new();
        for (k, c) in desc.columns.iter().enumerate().filter(|(k, _)| *k != at) {
            let Some(g) = c.generation() else { continue };
            let (e, _) = bind_generation(self, desc, (k, g))?;
            let mut used = Vec::new();
            expr_columns(&e, &mut used);
            if used.contains(&at) {
                on.push(c.name.clone());
            }
        }
        Ok(on)
    }

    /// [ISO/IEC 9075-2:2023 §11.23 SR7f] That no generated column of
    /// `desc` references its column `column`, at `at`, which is dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one does.
    pub(super) fn no_generated_on(&self, desc: &TableDesc, (at, column): (usize, &str)) -> Result<()> {
        if let Some(g) = self.generated_on(desc, at)?.first() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.23 SR7f] the generation expression of column {g} of {} references {column}, so it is dropped only with CASCADE",
                desc.name
            )));
        }
        Ok(())
    }
}

/// [ISO/IEC 9075-2:2023 §11.4 SR10, SR16, GR5f] How column `g` of table
/// `table`, of type `t`, its names of schema `names`, gives its values: of
/// an identity column, the sequence generator §9.36 makes of its options,
/// its epoch still to give.
///
/// # Errors
///
/// SQLCODE -103 if an identity column is not of an exact numeric type of
/// scale 0 \[SR16a\], or its options break §9.36's Syntax Rules.
fn generated_of(table: &QualifiedName, (names, t): (&str, DataType), g: &ColumnGeneration) -> Result<Generated> {
    match g {
        ColumnGeneration::Identity { always, options, at } => {
            if !matches!(t, DataType::Exact { scale: 0, .. }) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.4 SR16a] an identity column is of an exact numeric type of scale 0, not {t}"
                ))
                .or_at(*at));
            }
            Ok(Generated::Identity(Identity { always: *always, sequence: generator(table.clone(), t, options, *at)? }))
        }
        ColumnGeneration::Expression { text, .. } => Ok(Generated::Expression(Generation { text: text.clone(), names: names.to_owned() })),
    }
}

/// The generation expression of the column at `at` of `desc`, bound, and
/// its type.
///
/// # Errors
///
/// As `bind_generation`; SQLCODE -910 if the column is no generated column.
fn generation_type(st: &State, desc: &TableDesc, at: usize) -> Result<(crate::logical::Expr, DataType)> {
    let g = desc.columns.get(at).and_then(ColumnDesc::generation).cloned();
    let g = g.ok_or_else(|| super::api(format!("[DESIGN] column {at} of {} is no generated column", desc.name)))?;
    bind_generation(st, desc, (at, &g))
}

/// [ISO/IEC 9075-2:2023 §11.8 SR14] That the referential actions of the
/// foreign key `name` of table `child`, its referencing columns `columns`,
/// written at `at`, as `r` says, neither set a generated referencing column
/// null nor to its default, nor cascade an update to one.
///
/// # Errors
///
/// SQLCODE -103 if one does.
pub(super) fn generated_referencing(
    child: &TableDesc,
    columns: &[usize],
    r: &References,
    (name, at): (&QualifiedName, usize),
) -> Result<()> {
    let Some(column) = columns.iter().filter_map(|&c| child.columns.get(c)).find(|c| c.generation().is_some()) else { return Ok(()) };
    let sets = |a: ReferentialAction| matches!(a, ReferentialAction::SetNull | ReferentialAction::SetDefault);
    let cited = if sets(r.on_update) || sets(r.on_delete) {
        "SR14a] no <referential action> is SET NULL or SET DEFAULT"
    } else if r.on_update == ReferentialAction::Cascade {
        "SR14b] its <update rule> is not ON UPDATE CASCADE"
    } else {
        return Ok(());
    };
    let msg = format!("[ISO/IEC 9075-2:2023 §11.8 {cited} of {name}, as its referencing column {} is a generated column", column.name);
    Err(rule(msg).or_at(at))
}
