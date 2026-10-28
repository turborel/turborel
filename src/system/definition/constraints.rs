//! [ISO/IEC 9075:1992 §21.3.13-§21.3.18] The Definition Schema's tables of
//! table constraints: TABLE_CONSTRAINTS, KEY_COLUMN_USAGE,
//! REFERENTIAL_CONSTRAINTS, CHECK_CONSTRAINTS, CHECK_TABLE_USAGE and
//! CHECK_COLUMN_USAGE. A NOT NULL constraint is the CHECK constraint
//! `C IS NOT NULL` it is equivalent to [§11.4 SR9a]. Split from
//! definition.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::{Columns, defined, ids, long};
use crate::catalog::{
    Catalog, ColumnDesc, ConstraintDesc, ConstraintKind, ForeignKey, MatchType, QualifiedName, ReferentialAction, TableDesc,
};
use crate::error::Result;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, catalog_name, number, text, yes};
use crate::value::Value;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("TABLE_CONSTRAINTS", table_constraints_columns, Rows::Infallible(table_constraints)),
        defined("KEY_COLUMN_USAGE", key_columns_columns, Rows::Infallible(key_columns)),
        defined("REFERENTIAL_CONSTRAINTS", referential_columns, Rows::Infallible(referential)),
        defined("CHECK_CONSTRAINTS", checks_columns, Rows::Infallible(checks)),
        defined("CHECK_TABLE_USAGE", check_tables_columns, Rows::Fallible(check_tables)),
        defined("CHECK_COLUMN_USAGE", check_columns_columns, Rows::Fallible(check_columns)),
    ]
}

/// Each constraint of each base table, with its table.
fn constraints<'s>(seen: &Seen<'s>) -> Vec<(&'s TableDesc, &'s ConstraintDesc)> {
    // A copy of a domain constraint on a column is the domain's, not the
    // table's (`db/domains.rs`).
    let own = |t: &'s TableDesc| t.constraints.iter().filter(|c| c.domain.is_none()).map(move |c| (t, c));
    seen.state.catalog.tables().into_iter().flat_map(own).collect()
}

/// The name of column `at` of `table`.
fn column(table: &TableDesc, at: usize) -> Value {
    table.columns.get(at).map_or(Value::Null, |c| text(&c.name))
}

// ---------------------------------------------------------------------------
// [§21.3.13] TABLE_CONSTRAINTS
// ---------------------------------------------------------------------------

fn table_constraints_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("CONSTRAINT", true)
        .data("CONSTRAINT_TYPE", true)
        .ids("TABLE", true)
        .data("IS_DEFERRABLE", true)
        .data("INITIALLY_DEFERRED", true)
        .done()
}

/// Each, of its type, deferrable or not and initially deferred or not
/// [§10.6 GR1, GR2].
fn table_constraints(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    constraints(seen)
        .into_iter()
        .map(|(t, c)| {
            let kind = match &c.kind {
                ConstraintKind::Unique { primary: true, .. } => "PRIMARY KEY",
                ConstraintKind::Unique { primary: false, .. } => "UNIQUE",
                ConstraintKind::ForeignKey(_) => "FOREIGN KEY",
                ConstraintKind::NotNull(_) | ConstraintKind::Check { .. } => "CHECK",
            };
            let deferral = vec![yes(c.attributes.deferrable), yes(c.attributes.initially_deferred)];
            [ids(&catalog, &c.name).to_vec(), vec![text(kind)], ids(&catalog, &t.name).to_vec(), deferral].concat()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// [§21.3.14] KEY_COLUMN_USAGE, [§21.3.15] REFERENTIAL_CONSTRAINTS
// ---------------------------------------------------------------------------

fn key_columns_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("CONSTRAINT", true).ids("TABLE", true).id("COLUMN_NAME", true).number("ORDINAL_POSITION", true).done()
}

/// The UNIQUE or PRIMARY KEY constraint of the table `fk` references whose
/// columns are those it references [§11.8 SR2], and the columns'
/// positions, in the order that constraint names them.
fn referenced<'c>(catalog: &'c dyn Catalog, fk: &ForeignKey) -> Option<(&'c ConstraintDesc, &'c [usize])> {
    let mut wanted = fk.referenced.clone();
    wanted.sort_unstable();
    catalog.table(&fk.table)?.constraints.iter().find_map(|c| match &c.kind {
        ConstraintKind::Unique { columns, .. } => {
            let mut have = columns.clone();
            have.sort_unstable();
            (have == wanted).then_some((c, columns.as_slice()))
        }
        ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } => None,
    })
}

/// Each column of each UNIQUE, PRIMARY KEY and referential constraint, at
/// its position in it, from 1; a referencing column at the position in the
/// referenced unique constraint of the column it references \[Description
/// 3\].
fn key_columns(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for (t, c) in constraints(seen) {
        let positioned: Vec<(usize, usize)> = match &c.kind {
            ConstraintKind::Unique { columns, .. } => columns.iter().enumerate().map(|(k, &at)| (at, k)).collect(),
            ConstraintKind::ForeignKey(fk) => {
                let key = referenced(&*seen.state.catalog, fk).map_or(&[][..], |(_, columns)| columns);
                let place = |j: usize, r: &usize| key.iter().position(|k| k == r).unwrap_or(j);
                fk.columns.iter().zip(&fk.referenced).enumerate().map(|(j, (&at, r))| (at, place(j, r))).collect()
            }
            ConstraintKind::NotNull(_) | ConstraintKind::Check { .. } => Vec::new(),
        };
        for (at, k) in positioned {
            let named = [ids(&catalog, &c.name), ids(&catalog, &t.name)].concat();
            rows.push([named, vec![column(t, at), number(k.saturating_add(1))]].concat());
        }
    }
    rows
}

fn referential_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("CONSTRAINT", true)
        .ids("UNIQUE_CONSTRAINT", true)
        .data("MATCH_OPTION", true)
        .data("UPDATE_RULE", true)
        .data("DELETE_RULE", true)
        .done()
}

/// The `<referential action>` `a` as REFERENTIAL_CONSTRAINTS shows it
/// [Description 4, 5].
fn action(a: ReferentialAction) -> Value {
    text(match a {
        ReferentialAction::Cascade => "CASCADE",
        ReferentialAction::SetNull => "SET NULL",
        ReferentialAction::SetDefault => "SET DEFAULT",
        ReferentialAction::NoAction => "NO ACTION",
    })
}

/// Each referential constraint, the unique constraint it references, its
/// `<match type>`, NONE if none was written \[Description 3\], and its
/// rules.
fn referential(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for (_, c) in constraints(seen) {
        let ConstraintKind::ForeignKey(fk) = &c.kind else { continue };
        let Some((key, _)) = referenced(&*seen.state.catalog, fk) else { continue };
        let matching = text(match fk.match_type {
            MatchType::Simple => "NONE",
            MatchType::Partial => "PARTIAL",
            MatchType::Full => "FULL",
        });
        let rules = vec![matching, action(fk.on_update), action(fk.on_delete)];
        rows.push([ids(&catalog, &c.name).to_vec(), ids(&catalog, &key.name).to_vec(), rules].concat());
    }
    rows
}

// ---------------------------------------------------------------------------
// [§21.3.16] CHECK_CONSTRAINTS, [§21.3.17] CHECK_TABLE_USAGE,
// [§21.3.18] CHECK_COLUMN_USAGE
// ---------------------------------------------------------------------------

fn checks_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("CONSTRAINT", true).data("CHECK_CLAUSE", false).done()
}

/// Each CHECK constraint, its `<search condition>` as written, or that of
/// the CHECK a NOT NULL constraint is.
fn checks(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for (t, c) in constraints(seen) {
        let clause = match &c.kind {
            ConstraintKind::NotNull(at) => long(&format!("{} IS NOT NULL", t.columns.get(*at).map_or("", |col| col.name.as_str()))),
            ConstraintKind::Check { text: condition, .. } => long(condition),
            ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) => continue,
        };
        rows.push([ids(&catalog, &c.name).to_vec(), vec![clause]].concat());
    }
    // [§21.3.16] the domains' constraints too, and the assertions.
    let domains = seen.state.catalog.domains().into_iter().flat_map(|d| d.constraints.iter());
    for c in domains.chain(seen.state.catalog.assertions()) {
        if let ConstraintKind::Check { text: condition, .. } = &c.kind {
            rows.push([ids(&catalog, &c.name).to_vec(), vec![long(condition)]].concat());
        }
    }
    rows
}

/// The columns a constraint references, each as its table and column name.
type Uses = Vec<(QualifiedName, String)>;

/// The columns each CHECK constraint and assertion references, once each,
/// as its condition binds now, as its schema's owner's.
///
/// # Errors
///
/// A condition does not bind.
fn check_uses<'s>(seen: &Seen<'s>) -> Result<Vec<(&'s ConstraintDesc, Uses)>> {
    let mut all = Vec::new();
    for (t, c) in constraints(seen) {
        let uses = match &c.kind {
            ConstraintKind::NotNull(at) => t.columns.get(*at).map(|col| (t.name.clone(), col.name.clone())).into_iter().collect(),
            ConstraintKind::Check { text: condition, names, horizon, .. } => seen.state.check_uses(&t.name, (condition, names, horizon))?,
            ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) => continue,
        };
        all.push((c, uses));
    }
    for a in seen.state.catalog.assertions() {
        all.push((a, seen.state.assertion_uses(a)?));
    }
    Ok(all)
}

fn check_tables_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("CONSTRAINT", true).ids("TABLE", true).done()
}

/// Each table or view each CHECK constraint references, once: whose
/// columns it references, and those its subqueries name.
fn check_tables(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for (c, uses) in check_uses(seen)? {
        let reads = match &c.kind {
            ConstraintKind::Check { reads, .. } => reads.as_slice(),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) => &[],
        };
        let mut named: Vec<&QualifiedName> = Vec::new();
        for t in uses.iter().map(|(t, _)| t).chain(reads) {
            if !named.contains(&t) {
                named.push(t);
            }
        }
        rows.extend(named.into_iter().map(|t| [ids(&catalog, &c.name), ids(&catalog, t)].concat()));
    }
    Ok(rows)
}

fn check_columns_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("CONSTRAINT", true).ids("TABLE", true).id("COLUMN_NAME", true).done()
}

/// Each column each CHECK constraint references, once.
fn check_columns(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for (c, uses) in check_uses(seen)? {
        for (t, column) in uses {
            rows.push([ids(&catalog, &c.name).to_vec(), ids(&catalog, &t).to_vec(), vec![text(&column)]].concat());
        }
    }
    Ok(rows)
}
