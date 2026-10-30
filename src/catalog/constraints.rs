//! \[EXTENSION\] Table constraints [ISO/IEC 9075:1992 §11.6]: their
//! descriptors in the catalog, each named [§10.6]. SQL-86's NOT NULL and
//! UNIQUE [X3.135-1986 §6.3, §6.4] are among them. A domain's constraints
//! [§11.21] and an assertion [§11.34], each a CHECK, are described the
//! same way. Split from catalog.rs
//! (RULES.md R-SIZE-2).

use std::fmt;

use super::{QualifiedName, TableDesc};
pub use crate::ast::{ConstraintAttributes, MatchType, ReferentialAction};

/// A table constraint, a domain constraint or an assertion, named [ISO/IEC
/// 9075:1992 §10.6].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintDesc {
    /// Its `<constraint name>`, in its table's, domain's or assertion's
    /// schema [§10.6 SR1, SR2].
    pub name: QualifiedName,
    /// What it constrains.
    pub kind: ConstraintKind,
    /// Whether it is deferrable, and its initial constraint mode
    /// [§10.6 GR1, GR2].
    pub attributes: ConstraintAttributes,
    /// [§11.21 GR5, GR6] Of a CHECK that is a domain constraint on one of
    /// the table's columns, that constraint's name: it is the domain's,
    /// kept as the domain changes (`db/domains.rs`).
    pub domain: Option<QualifiedName>,
}

/// What a table constraint constrains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintKind {
    /// [§11.4] NOT NULL, on the column at this position.
    NotNull(usize),
    /// [§11.7] UNIQUE, or PRIMARY KEY if `primary`, on the columns at
    /// these positions: no two rows have equal values in all of them, nulls
    /// not counting [§8.9]; a PRIMARY KEY's are not null \[SR3a\].
    Unique {
        /// The columns' positions, in the order written.
        columns: Vec<usize>,
        /// PRIMARY KEY.
        primary: bool,
    },
    /// [§11.8] A referential constraint.
    ForeignKey(ForeignKey),
    /// [§11.9] CHECK: its `<search condition>` as written, over the
    /// table's columns; it holds unless false for a row [§11.6 GR3].
    Check {
        /// The `<search condition>`.
        text: String,
        /// The schema of the names in `text` written without one [§5.4
        /// SR8]: its table's, domain's or assertion's, in a `<schema
        /// definition>`; else the session's default schema as it was
        /// defined.
        names: String,
        /// The tables its subqueries read, none if it has none: a change
        /// to one, or to the table, may falsify it for any row.
        reads: Vec<QualifiedName>,
        /// The columns of its table and of the tables it reads it sees,
        /// once it is bound: none before.
        horizon: super::Horizon,
    },
}

/// [ISO/IEC 9075:1992 §11.8] A referential constraint: the referencing
/// columns, and the referenced table and columns, the i-th of each
/// corresponding \[SR7\].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    /// The positions of the referencing columns.
    pub columns: Vec<usize>,
    /// The referenced table.
    pub table: QualifiedName,
    /// The positions, in it, of the referenced columns.
    pub referenced: Vec<usize>,
    /// Its `<match type>` \[GR2\].
    pub match_type: MatchType,
    /// The `<update rule>` \[SR8\].
    pub on_update: ReferentialAction,
    /// The `<delete rule>` \[SR9\].
    pub on_delete: ReferentialAction,
}

impl fmt::Display for ConstraintDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl TableDesc {
    /// The column positions of each UNIQUE and PRIMARY KEY constraint.
    pub fn unique(&self) -> impl Iterator<Item = &[usize]> {
        self.constraints.iter().filter_map(|c| match &c.kind {
            ConstraintKind::Unique { columns, .. } => Some(columns.as_slice()),
            ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } => None,
        })
    }

    /// The columns of its PRIMARY KEY, if it has one.
    pub fn primary_key(&self) -> Option<&[usize]> {
        self.constraints.iter().find_map(|c| match &c.kind {
            ConstraintKind::Unique { columns, primary: true } => Some(columns.as_slice()),
            ConstraintKind::Unique { primary: false, .. }
            | ConstraintKind::NotNull(_)
            | ConstraintKind::ForeignKey(_)
            | ConstraintKind::Check { .. } => None,
        })
    }

    /// Its referential constraints, with their names.
    pub fn foreign_keys(&self) -> impl Iterator<Item = (&QualifiedName, &ForeignKey)> {
        self.constraints.iter().filter_map(|c| match &c.kind {
            ConstraintKind::ForeignKey(fk) => Some((&c.name, fk)),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::Check { .. } => None,
        })
    }

    /// Its CHECK constraints' conditions, as written, with their names.
    pub fn checks(&self) -> impl Iterator<Item = (&QualifiedName, &str)> {
        self.constraints.iter().filter_map(|c| match &c.kind {
            ConstraintKind::Check { text, .. } => Some((&c.name, text.as_str())),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) => None,
        })
    }

    /// Sets each column's `not_null` from the constraints: NOT NULL, or
    /// part of the PRIMARY KEY [ISO/IEC 9075:1992 §11.7 SR3a].
    pub fn refresh_not_null(&mut self) {
        let primary: Vec<usize> = self.primary_key().map(<[usize]>::to_vec).unwrap_or_default();
        let not_null: Vec<usize> = self
            .constraints
            .iter()
            .filter_map(|c| if let ConstraintKind::NotNull(at) = c.kind { Some(at) } else { None })
            .chain(primary)
            .collect();
        for (at, column) in self.columns.iter_mut().enumerate() {
            column.not_null = not_null.contains(&at);
        }
    }
}
