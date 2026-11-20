//! \[EXTENSION\] Table constraints [ISO/IEC 9075:1992 §11.6]: their
//! descriptors in the catalog, each named [§10.6]. SQL-86's NOT NULL and
//! UNIQUE [X3.135-1986 §6.3, §6.4] are among them. A domain's constraints
//! [§11.21] and an assertion [§11.34], each a CHECK, are described the
//! same way.

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
        /// [ISO/IEC 9075-2:2023 §4.25.3.2, §11.7 SR5] NULLS DISTINCT, as
        /// a PRIMARY KEY's is: two rows of a null among their values are
        /// unique. Else NULLS NOT DISTINCT, a null in one equal to a null
        /// in the other.
        nulls_distinct: bool,
        /// [ISO/IEC 9075-2:2023 §4.25.3.2, §11.7 SR6d i] The application-
        /// time period it is WITHOUT OVERLAPS of, if any: then two rows
        /// are of equal values only if their periods overlap, and its
        /// index's key is its columns, then the period's start and end
        /// columns (`TableDesc::key_columns`).
        period: Option<String>,
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
        /// The tables its subqueries read, none if it has none, its own
        /// table if they read none: a change to one, or to the table, may
        /// falsify it for any row.
        reads: Vec<QualifiedName>,
        /// The columns of its table and of the tables it reads it sees,
        /// once it is bound: none before.
        horizon: super::Horizon,
    },
    /// \[EXTENSION\] (§4.2) CONNECTION, of an edge table: the node tables
    /// its edges may join.
    Connection(super::Connection),
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
    /// [ISO/IEC 9075-2:2023 §4.25.3.3, §11.8 GR2d, GR2e] Of a temporal
    /// one, the names of its referencing period, of its table, and of the
    /// referenced period: a row's period is then covered by the periods of
    /// the referenced rows its values match.
    pub period: Option<(String, String)>,
}

impl ForeignKey {
    /// The columns of `child`, its table, whose values it judges: its
    /// referencing columns, and of a temporal one its period's start and
    /// end [ISO/IEC 9075-2:2023 §11.8 SR18a i].
    pub fn referencing_key(&self, child: &TableDesc) -> Vec<usize> {
        let mut key = self.columns.clone();
        key.extend(self.period.as_ref().and_then(|(own, _)| child.period_columns(own)).into_iter().flatten());
        key
    }

    /// The columns of `parent`, the table it references, whose values it
    /// judges: the referenced columns, and of a temporal one its period's.
    pub fn referenced_key(&self, parent: &TableDesc) -> Vec<usize> {
        let mut key = self.referenced.clone();
        key.extend(self.period.as_ref().and_then(|(_, theirs)| parent.period_columns(theirs)).into_iter().flatten());
        key
    }
}

impl fmt::Display for ConstraintDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl TableDesc {
    /// The key of the index of each UNIQUE and PRIMARY KEY constraint, no
    /// two rows' the same (`key_columns`).
    pub fn unique(&self) -> impl Iterator<Item = Vec<usize>> + '_ {
        self.constraints.iter().filter_map(|c| match &c.kind {
            ConstraintKind::Unique { columns, period, .. } => Some(self.key_columns(columns, period.as_deref())),
            ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } | ConstraintKind::Connection(_) => {
                None
            }
        })
    }

    /// The positions of the start and end columns of its period `name`, if
    /// it has it.
    pub fn period_columns(&self, name: &str) -> Option<[usize; 2]> {
        let period = self.periods.iter().find(|p| p.name == name)?;
        Some([self.column(&period.start)?, self.column(&period.end)?])
    }

    /// The key of the index of a unique constraint on `columns`: them, and
    /// of one WITHOUT OVERLAPS of `period`, its start and end columns
    /// after, a key no two rows have once none overlap [ISO/IEC
    /// 9075-2:2023 §11.7 SR6d i].
    pub fn key_columns(&self, columns: &[usize], period: Option<&str>) -> Vec<usize> {
        let mut key = columns.to_vec();
        key.extend(period.and_then(|n| self.period_columns(n)).into_iter().flatten());
        key
    }

    /// Whether a UNIQUE NULLS NOT DISTINCT constraint is on exactly
    /// `columns` [ISO/IEC 9075-2:2023 §11.7], so their index holds the keys
    /// with a null (`IndexDesc::nulls`).
    pub fn nulls_not_distinct(&self, columns: &[usize]) -> bool {
        self.constraints.iter().any(|c| match &c.kind {
            ConstraintKind::Unique { columns: on, nulls_distinct, period, .. } => {
                !nulls_distinct && self.key_columns(on, period.as_deref()) == columns
            }
            ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } | ConstraintKind::Connection(_) => {
                false
            }
        })
    }

    /// The columns of its PRIMARY KEY, if it has one.
    pub fn primary_key(&self) -> Option<&[usize]> {
        self.constraints.iter().find_map(|c| match &c.kind {
            ConstraintKind::Unique { columns, primary: true, .. } => Some(columns.as_slice()),
            ConstraintKind::Unique { primary: false, .. }
            | ConstraintKind::NotNull(_)
            | ConstraintKind::ForeignKey(_)
            | ConstraintKind::Check { .. }
            | ConstraintKind::Connection(_) => None,
        })
    }

    /// Its referential constraints, with their names.
    pub fn foreign_keys(&self) -> impl Iterator<Item = (&QualifiedName, &ForeignKey)> {
        self.constraints.iter().filter_map(|c| match &c.kind {
            ConstraintKind::ForeignKey(fk) => Some((&c.name, fk)),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::Check { .. } | ConstraintKind::Connection(_) => {
                None
            }
        })
    }

    /// Its CHECK constraints' conditions, as written, with their names.
    pub fn checks(&self) -> impl Iterator<Item = (&QualifiedName, &str)> {
        self.constraints.iter().filter_map(|c| match &c.kind {
            ConstraintKind::Check { text, .. } => Some((&c.name, text.as_str())),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) | ConstraintKind::Connection(_) => {
                None
            }
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

/// \[DESIGN\] What a statement's end looks for over every table of a
/// catalog, found once for its tables (RULES.md D415): whether one has a
/// foreign key, whose referenced rows a change may break, whether one
/// has a CHECK constraint that reads tables, which a change of them may
/// break, and whether one is a node table, whose rows deleted may have
/// edges to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Constrained {
    /// A table has a foreign key.
    pub foreign_keys: bool,
    /// A table has a CHECK constraint that reads tables.
    pub reading_checks: bool,
    /// A table is a node table.
    pub nodes: bool,
}

impl Constrained {
    /// Of the tables `tables`.
    pub fn of<'a>(tables: impl Iterator<Item = &'a TableDesc>) -> Constrained {
        let mut found = Constrained::default();
        let tables: Vec<&TableDesc> = tables.collect();
        found.nodes = tables.iter().any(|t| t.native() == Some(super::NativeKind::Node));
        for c in tables.iter().flat_map(|t| t.constraints.iter()) {
            found.foreign_keys |= matches!(c.kind, ConstraintKind::ForeignKey(_));
            if let ConstraintKind::Check { reads, .. } = &c.kind {
                found.reading_checks |= !reads.is_empty();
            }
        }
        found
    }
}
