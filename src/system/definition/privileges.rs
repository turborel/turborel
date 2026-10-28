//! [ISO/IEC 9075:1992 §21.3.3, §21.3.20-§21.3.22] The Definition Schema's
//! tables of users and privileges: USERS, TABLE_PRIVILEGES,
//! COLUMN_PRIVILEGES and USAGE_PRIVILEGES. Split from definition.rs
//! (RULES.md R-SIZE-2).
//!
//! The privilege descriptors are those of the owners, of the grantor
//! "_SYSTEM" [§4.26]: every privilege on a base table and on each of its
//! columns, grantable [§11.3 GR4]; on a view, those its owner holds
//! [X3.135-1986 §6.6 SR3b i]; those of each `<grant statement>`; and on the
//! Information Schema, SELECT to PUBLIC, WITH GRANT OPTION, of the grantor
//! INFORMATION_SCHEMA \[§21.1\]. A table privilege but DELETE is also one on
//! each column [§11.3 GR4, §11.36 GR4]; UPDATE or REFERENCES on every
//! column is the table privilege too, there being no other record of one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::{Columns, Object, defined, ids, objects, schemas, sql_text, usage};
use crate::catalog::{Actions, ColumnDesc, QualifiedName, applicable, owner_of};
use crate::error::Result;
use crate::storage::Row;
use crate::system::{Rows, SYSTEM_GRANTOR, Seen, SystemTable, catalog_name, text, yes};

/// [§21.3.20 Description 2] The grantee of a privilege granted to all.
const PUBLIC: &str = "PUBLIC";

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("USERS", users_columns, Rows::Fallible(users)),
        defined("TABLE_PRIVILEGES", table_privileges_columns, Rows::Fallible(table_privileges)),
        defined("COLUMN_PRIVILEGES", column_privileges_columns, Rows::Fallible(column_privileges)),
        defined("USAGE_PRIVILEGES", usage_privileges_columns, Rows::Infallible(usage_privileges)),
    ]
}

/// A privilege descriptor: its grantor, grantee, table, privilege, and
/// column if it is a column privilege's.
type Descriptor = (String, String, QualifiedName, &'static str, Option<String>);

/// Every privilege descriptor, each once, and whether it is grantable.
///
/// # Errors
///
/// A view of the Information Schema does not bind: a defect.
fn descriptors(seen: &Seen<'_>) -> Result<BTreeMap<Descriptor, bool>> {
    let catalog = &*seen.state.catalog;
    let mut all = BTreeMap::new();
    for o in objects(seen)? {
        let owned = catalog.table(&o.name).is_some() || catalog.view(&o.name).is_some();
        if !owned {
            let select = Actions { select: true, ..Actions::default() };
            add(&mut all, (SYSTEM_GRANTOR, &o.name.auth, &o), (&select, &select));
            add(&mut all, (&o.name.auth, PUBLIC, &o), (&select, &select));
            continue;
        }
        let schema_owner = owner_of(catalog, &o.name.auth);
        let (held, grantable) = applicable(catalog, schema_owner, &o.name);
        add(&mut all, (SYSTEM_GRANTOR, schema_owner, &o), (&held, &grantable));
        for p in catalog.privileges(&o.name) {
            let given = if p.grantable { p.actions.clone() } else { Actions::default() };
            add(&mut all, (&p.grantor, p.grantee.as_deref().unwrap_or(PUBLIC), &o), (&p.actions, &given));
        }
    }
    Ok(all)
}

/// Adds to `all` the descriptors of `held` on `object`, granted by
/// `grantor` to `grantee`, grantable as `grantable` says.
fn add(all: &mut BTreeMap<Descriptor, bool>, (grantor, grantee, object): (&str, &str, &Object), (held, grantable): (&Actions, &Actions)) {
    let width = object.columns.len();
    let every = |columns: &BTreeSet<usize>| width > 0 && (0..width).all(|c| columns.contains(&c));
    let table = [
        ("SELECT", held.select, grantable.select),
        ("INSERT", every(&held.insert), every(&grantable.insert)),
        ("DELETE", held.delete, grantable.delete),
        ("UPDATE", every(&held.update), every(&grantable.update)),
        ("REFERENCES", every(&held.references), every(&grantable.references)),
    ];
    let mut put = |kind: &'static str, column: Option<String>, can: bool| {
        let key = (grantor.to_owned(), grantee.to_owned(), object.name.clone(), kind, column);
        let was = all.get(&key).copied().unwrap_or(false);
        all.insert(key, was || can);
    };
    for (kind, _, can) in table.into_iter().filter(|t| t.1) {
        put(kind, None, can);
    }
    for (at, c) in object.columns.iter().enumerate() {
        let columns = [
            ("SELECT", held.select, grantable.select),
            ("INSERT", held.insert.contains(&at), grantable.insert.contains(&at)),
            ("UPDATE", held.update.contains(&at), grantable.update.contains(&at)),
            ("REFERENCES", held.references.contains(&at), grantable.references.contains(&at)),
        ];
        for (kind, _, can) in columns.into_iter().filter(|t| t.1) {
            put(kind, Some(c.name.clone()), can);
        }
    }
}

// ---------------------------------------------------------------------------
// [§21.3.3] USERS
// ---------------------------------------------------------------------------

fn users_columns() -> Vec<ColumnDesc> {
    Columns::of().id("USER_NAME", true).done()
}

/// Every `<authorization identifier>` known: of each schema's owner, and
/// each grantor and grantee, "_SYSTEM" and PUBLIC among them, in order.
fn users(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let mut known: BTreeSet<String> = schemas(seen).into_iter().collect();
    known.extend([SYSTEM_GRANTOR.to_owned(), PUBLIC.to_owned()]);
    for (grantor, grantee, ..) in descriptors(seen)?.into_keys() {
        known.extend([grantor, grantee]);
    }
    Ok(known.iter().map(|u| vec![text(u)]).collect())
}

// ---------------------------------------------------------------------------
// [§21.3.20] TABLE_PRIVILEGES, [§21.3.21] COLUMN_PRIVILEGES
// ---------------------------------------------------------------------------

fn table_privileges_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .id("GRANTOR", true)
        .id("GRANTEE", true)
        .ids("TABLE", true)
        .data(("PRIVILEGE_TYPE", 10), true)
        .data(("IS_GRANTABLE", 3), true)
        .done()
}

/// Each table privilege descriptor.
fn table_privileges(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    Ok(descriptors(seen)?
        .into_iter()
        .filter(|((.., column), _)| column.is_none())
        .map(|((grantor, grantee, table, kind, _), grantable)| {
            [vec![text(&grantor), text(&grantee)], ids(&catalog, &table).to_vec(), vec![text(kind), yes(grantable)]].concat()
        })
        .collect())
}

fn column_privileges_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .id("GRANTOR", true)
        .id("GRANTEE", true)
        .ids("TABLE", true)
        .id("COLUMN_NAME", true)
        .data(("PRIVILEGE_TYPE", 10), true)
        .data(("IS_GRANTABLE", 3), true)
        .done()
}

/// Each column privilege descriptor.
fn column_privileges(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    Ok(descriptors(seen)?
        .into_iter()
        .filter_map(|((grantor, grantee, table, kind, column), grantable)| {
            let column = text(&column?);
            Some([vec![text(&grantor), text(&grantee)], ids(&catalog, &table).to_vec(), vec![column, text(kind), yes(grantable)]].concat())
        })
        .collect())
}

// ---------------------------------------------------------------------------
// [§21.3.22] USAGE_PRIVILEGES
// ---------------------------------------------------------------------------

fn usage_privileges_columns() -> Vec<ColumnDesc> {
    Columns::of().id("GRANTOR", true).id("GRANTEE", true).ids("OBJECT", true).id("OBJECT_TYPE", true).data(("IS_GRANTABLE", 3), true).done()
}

/// USAGE on the character set and collation SQL_TEXT (`definition::usage`);
/// and on each domain, its owner's [§11.21 GR4] and each granted.
fn usage_privileges(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows: Vec<Row> = usage()
        .into_iter()
        .map(|(kind, grantable)| {
            [vec![text(SYSTEM_GRANTOR), text(PUBLIC)], ids(&catalog, &sql_text()).to_vec(), vec![text(kind), yes(grantable)]].concat()
        })
        .collect();
    for d in seen.state.catalog.domains() {
        let row = |grantor: &str, grantee: &str, grantable: bool| {
            [vec![text(grantor), text(grantee)], ids(&catalog, &d.name).to_vec(), vec![text("DOMAIN"), yes(grantable)]].concat()
        };
        rows.push(row(SYSTEM_GRANTOR, owner_of(&*seen.state.catalog, &d.name.auth), true));
        rows.extend(d.usage.iter().map(|u| row(&u.grantor, u.grantee.as_deref().unwrap_or(PUBLIC), u.grantable)));
    }
    rows
}
