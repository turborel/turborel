//! [ISO/IEC 9075:1992 §21.3] The Definition Schema, DEFINITION_SCHEMA:
//! system tables whose rows describe what the catalog holds, which the
//! views of the Information Schema read (`system/information.rs`). "An
//! implementation need do no more than simulate the existence of the
//! Definition Schema, as viewed through the Information Schema views"
//! \[§21.1\]: each table's rows are made from the definitions as the view
//! reads it, and no statement but such a view's names one \[§21.1\].
//!
//! Each table has the columns §21.3 defines, of the domains of §21.2.27 to
//! §21.2.29: SQL_IDENTIFIER, CHARACTER VARYING(128) (`system::identifier`);
//! CARDINAL_NUMBER, INTEGER; CHARACTER_DATA, CHARACTER VARYING(ML), ML
//! implementation-defined \[§21.2.28\], here the longest character string,
//! 32767 characters: a text longer is shown as the null value, as
//! VIEW_DEFINITION's and CHECK_CLAUSE's are when they "cannot be
//! represented without truncation" [§21.3.9 Description 3, §21.3.16
//! Description 2]. There are no translations, so TRANSLATIONS has no
//! rows. SQL_LANGUAGES has one, of the conformance claimed: to ISO 9075
//! of 1987, the text of X3.135-1986, at Level 2, by direct invocation
//! (CONFORMANCE.md) [§21.3.26].
//!
//! The objects described are the base tables and views of the catalog,
//! the system tables but the Definition Schema's own, and the views of the
//! Information Schema; and the one character set, SQL_TEXT [§10.4 SR3],
//! and its collation [§21.3.23, §21.3.24]. The constraint tables are in
//! `definition/constraints.rs`, the privilege tables and USERS in
//! `definition/privileges.rs`, and \[EXTENSION\] the tables of table and
//! index options and of indexes in `definition/options.rs` (RULES.md
//! R-SIZE-2).

mod constraints;
mod options;
mod privileges;
#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};

use super::{
    DEFINITION_SCHEMA, Defined, INFORMATION_SCHEMA, Rows, Seen, SystemTable, cardinal, catalog_name, desc_of, identifier, number, text,
    text_column, yes,
};
use crate::binder::{system_view_columns, view_uses};
use crate::catalog::{CheckLevels, ColumnDefault, ColumnDesc, ConstraintDesc, QualifiedName, TemporaryKind, owner_of};
use crate::error::Result;
use crate::storage::Row;
use crate::value::datetime::Moment;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075:1992 §21.2.28] ML, the length of CHARACTER_DATA: the
/// longest character string.
pub(super) const LONG: u32 = crate::value::MAX_CHARACTER_LENGTH;

/// [ISO/IEC 9075:1992 §10.4 SR3, §21.3.23 Description 5] The one
/// character set, and its collation [§21.3.24 Description 4]: SQL_TEXT,
/// of INFORMATION_SCHEMA.
const SQL_TEXT: &str = "SQL_TEXT";

/// [ISO/IEC 9075:1992 §21.3.23 Description 5b, 5c] Its form-of-use and
/// number of characters, implementation-defined: a character string is
/// Unicode, every scalar value of which is a character.
const FORM_OF_USE: &str = "UTF-8";
const NUMBER_OF_CHARACTERS: usize = 1_112_064;

/// The Definition Schema's tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    let mut tables = vec![
        defined("SCHEMATA", schemata_columns, Rows::Infallible(schemata)),
        defined("DATA_TYPE_DESCRIPTOR", data_type_columns, Rows::Fallible(data_types)),
        defined("DOMAINS", domains_columns, Rows::Infallible(domains)),
        defined("DOMAIN_CONSTRAINTS", domain_constraints_columns, Rows::Infallible(domain_constraints)),
        defined("TABLES", tables_columns, Rows::Fallible(described_tables)),
        defined("VIEWS", views_columns, Rows::Infallible(views)),
        defined("COLUMNS", columns_columns, Rows::Fallible(columns)),
        defined("VIEW_TABLE_USAGE", view_table_columns, Rows::Infallible(view_tables)),
        defined("VIEW_COLUMN_USAGE", view_column_columns, Rows::Fallible(view_columns)),
        defined("ASSERTIONS", assertions_columns, Rows::Infallible(assertions)),
        defined("CHARACTER_SETS", character_sets_columns, Rows::Infallible(character_sets)),
        defined("COLLATIONS", collations_columns, Rows::Infallible(collations)),
        defined("TRANSLATIONS", translations_columns, Rows::Infallible(none)),
        defined("SQL_LANGUAGES", languages_columns, Rows::Infallible(languages)),
    ];
    tables.extend(constraints::tables());
    tables.extend(privileges::tables());
    tables.extend(options::tables());
    tables
}

/// The Definition Schema's table `name`.
pub(super) fn defined(name: &'static str, columns: fn() -> Vec<ColumnDesc>, rows: Rows) -> Arc<dyn SystemTable> {
    Arc::new(Defined { schema: DEFINITION_SCHEMA, name, columns, rows })
}

/// The rows of a table of what there is none of.
fn none(_: &Seen<'_>) -> Vec<Row> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Columns and values
// ---------------------------------------------------------------------------

/// The columns of a table of the Definition Schema, in order.
pub(super) struct Columns(Vec<ColumnDesc>);

impl Columns {
    /// None yet.
    pub(super) fn of() -> Columns {
        Columns(Vec::new())
    }

    /// Then a column `name` of SQL_IDENTIFIER, NOT NULL if `not_null`.
    pub(super) fn id(mut self, name: &str, not_null: bool) -> Columns {
        self.0.push(identifier(name, not_null));
        self
    }

    /// Then `prefix`_CATALOG, `prefix`_SCHEMA and `prefix`_NAME, of
    /// SQL_IDENTIFIER: the name of an object.
    pub(super) fn ids(self, prefix: &str, not_null: bool) -> Columns {
        ["CATALOG", "SCHEMA", "NAME"].iter().fold(self, |c, part| c.id(&format!("{prefix}_{part}"), not_null))
    }

    /// Then a column `name` of CARDINAL_NUMBER.
    pub(super) fn number(mut self, name: &str, not_null: bool) -> Columns {
        self.0.push(cardinal(name, not_null));
        self
    }

    /// Then a column `name` of CHARACTER_DATA.
    pub(super) fn data(mut self, name: &str, not_null: bool) -> Columns {
        self.0.push(text_column((name, LONG), not_null));
        self
    }

    /// The columns.
    pub(super) fn done(self) -> Vec<ColumnDesc> {
        self.0
    }
}

/// The catalog, schema and name of object `name`, of the catalog `catalog`.
pub(super) fn ids(catalog: &Value, name: &QualifiedName) -> [Value; 3] {
    [catalog.clone(), text(&name.auth), text(&name.ident)]
}

/// The name of an object of INFORMATION_SCHEMA.
pub(super) fn information(ident: &str) -> QualifiedName {
    QualifiedName { auth: INFORMATION_SCHEMA.to_owned(), ident: ident.to_owned() }
}

/// `s` in a column of CHARACTER_DATA: the null value if it is longer.
pub(super) fn long(s: &str) -> Value {
    if s.chars().count() > usize::try_from(LONG).unwrap_or(usize::MAX) { Value::Null } else { text(s) }
}

/// A whole number of a CARDINAL_NUMBER column.
fn count(n: u32) -> Value {
    number(usize::try_from(n).unwrap_or(usize::MAX))
}

// ---------------------------------------------------------------------------
// The objects described
// ---------------------------------------------------------------------------

/// A table the Definition Schema describes: its name, whether it is a
/// view, and its columns.
pub(super) struct Object {
    pub(super) name: QualifiedName,
    pub(super) view: bool,
    pub(super) columns: Vec<ColumnDesc>,
}

/// [ISO/IEC 9075:1992 §21.3.8] Every table: the base tables and views of
/// the catalog; the system tables, the Definition Schema's own not among
/// them, in order of name; and the views of the Information Schema.
///
/// # Errors
///
/// A view of the Information Schema does not bind: a defect.
pub(super) fn objects(seen: &Seen<'_>) -> Result<Vec<Object>> {
    let st = seen.state;
    let tables = st.catalog.tables().into_iter().map(|t| Object { name: t.name.clone(), view: false, columns: t.columns.clone() });
    let views = st.catalog.views().into_iter().map(|v| Object { name: v.name.clone(), view: true, columns: v.columns.clone() });
    let mut system: Vec<Object> = st
        .system
        .values()
        .filter(|t| t.schema() != DEFINITION_SCHEMA)
        .map(|t| {
            let desc = desc_of(&**t);
            Object { name: desc.name, view: false, columns: desc.columns }
        })
        .collect();
    system.sort_by(|a, b| (&a.name.auth, &a.name.ident).cmp(&(&b.name.auth, &b.name.ident)));
    let mut all: Vec<Object> = tables.chain(views).chain(system).collect();
    for (v, columns) in super::information::views().iter().zip(information_columns(seen)?) {
        all.push(Object { name: information(v.name), view: true, columns });
    }
    Ok(all)
}

/// The columns of each view of the Information Schema, in order. They are
/// those of its query over the Definition Schema, whose columns are
/// constant, so the same in every database: found once, binding them.
///
/// # Errors
///
/// A view does not bind: a defect.
fn information_columns(seen: &Seen<'_>) -> Result<Vec<Vec<ColumnDesc>>> {
    static FOUND: OnceLock<Vec<Vec<ColumnDesc>>> = OnceLock::new();
    if let Some(found) = FOUND.get() {
        return Ok(found.clone());
    }
    let found = super::information::views().iter().map(|v| system_view_columns(seen.state, v)).collect::<Result<Vec<_>>>()?;
    Ok(FOUND.get_or_init(|| found).clone())
}

/// [ISO/IEC 9075:1992 §21.3.4] Every schema: the catalog's, in the order
/// created, then the system tables', in order of name.
pub(super) fn schemas(seen: &Seen<'_>) -> Vec<String> {
    let mut all: Vec<String> = seen.state.catalog.schemas().into_iter().map(str::to_owned).collect();
    let mut system: Vec<String> = seen.state.system.keys().map(|t| t.auth.clone()).filter(|s| !all.contains(s)).collect();
    system.sort_unstable();
    system.dedup();
    all.extend(system);
    all
}

// ---------------------------------------------------------------------------
// [§21.3.4] SCHEMATA
// ---------------------------------------------------------------------------

fn schemata_columns() -> Vec<ColumnDesc> {
    Columns::of().id("CATALOG_NAME", true).id("SCHEMA_NAME", true).id("SCHEMA_OWNER", true).ids("DEFAULT_CHARACTER_SET", false).done()
}

/// Each schema, with its owner [ISO/IEC 9075:1992 §11.1 GR2]
/// (`owner_of`), its default character set SQL_TEXT.
fn schemata(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let set = ids(&catalog, &information(SQL_TEXT));
    let owner = |s: &str| text(owner_of(&*seen.state.catalog, s));
    schemas(seen).iter().map(|s| [vec![catalog.clone(), text(s), owner(s)], set.to_vec()].concat()).collect()
}

// ---------------------------------------------------------------------------
// [§21.3.5] DATA_TYPE_DESCRIPTOR
// ---------------------------------------------------------------------------

fn data_type_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE_OR_DOMAIN", true)
        .id("COLUMN_NAME", true)
        .data("DATA_TYPE", true)
        .number("CHARACTER_MAXIMUM_LENGTH", false)
        .number("CHARACTER_OCTET_LENGTH", false)
        .ids("COLLATION", false)
        .number("NUMERIC_PRECISION", false)
        .number("NUMERIC_PRECISION_RADIX", false)
        .number("NUMERIC_SCALE", false)
        .number("DATETIME_PRECISION", false)
        .done()
}

/// One row for each domain, its column name empty, and one for each
/// column of each table that is of no domain [§21.3.5].
fn data_types(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for d in seen.state.catalog.domains() {
        rows.push([ids(&catalog, &d.name).to_vec(), vec![text("")], data_type(&catalog, d.data_type)].concat());
    }
    for o in objects(seen)? {
        for c in o.columns.iter().filter(|c| c.domain.is_none()) {
            rows.push([ids(&catalog, &o.name).to_vec(), vec![text(&c.name)], data_type(&catalog, c.data_type)].concat());
        }
    }
    Ok(rows)
}

/// [§21.3.5 Description 3] The data type `t`, from DATA_TYPE on, as its
/// kind describes it (`SqlType::descriptor`): a character string's
/// collation SQL_TEXT, the one there is.
fn data_type(catalog: &Value, t: DataType) -> Vec<Value> {
    let d = t.kind().descriptor(&t);
    let optional = |n: Option<u32>| n.map_or(Value::Null, count);
    let collation = if d.collated { ids(catalog, &information(SQL_TEXT)).to_vec() } else { vec![Value::Null, Value::Null, Value::Null] };
    let octets = d.octets.map_or(Value::Null, number);
    let head = vec![text(&d.name), optional(d.length), octets];
    let tail = vec![optional(d.precision), optional(d.radix), optional(d.scale), optional(d.datetime_precision)];
    [head, collation, tail].concat()
}

// ---------------------------------------------------------------------------
// [§21.3.6] DOMAINS, [§21.3.7] DOMAIN_CONSTRAINTS
// ---------------------------------------------------------------------------

fn domains_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("DOMAIN", true).data("DOMAIN_DEFAULT", false).done()
}

/// Each domain, with its default as its `<default option>` is written.
fn domains(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    seen.state
        .catalog
        .domains()
        .into_iter()
        .map(|d| [ids(&catalog, &d.name).to_vec(), vec![default_text(d.default.as_ref())]].concat())
        .collect()
}

fn domain_constraints_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("CONSTRAINT", true).ids("DOMAIN", true).data("IS_DEFERRABLE", true).data("INITIALLY_DEFERRED", true).done()
}

/// Each constraint of each domain, deferrable or not and initially
/// deferred or not.
fn domain_constraints(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for d in seen.state.catalog.domains() {
        for c in &d.constraints {
            let deferral = vec![yes(c.attributes.deferrable), yes(c.attributes.initially_deferred)];
            rows.push([ids(&catalog, &c.name).to_vec(), ids(&catalog, &d.name).to_vec(), deferral].concat());
        }
    }
    rows
}

/// A default as its `<default option>` is written [§11.5]; none, null.
fn default_text(default: Option<&ColumnDefault>) -> Value {
    match default {
        None => Value::Null,
        Some(ColumnDefault::Value(v) | ColumnDefault::Padded(v)) => long(&v.to_literal()),
        Some(ColumnDefault::User) => text("CURRENT_USER"),
        Some(ColumnDefault::Datetime(moment, precision)) => text(&match moment {
            Moment::Date => "CURRENT_DATE".to_owned(),
            Moment::Time => format!("CURRENT_TIME({precision})"),
            Moment::Timestamp => format!("CURRENT_TIMESTAMP({precision})"),
        }),
    }
}

// ---------------------------------------------------------------------------
// [§21.3.19] ASSERTIONS, [§21.3.25] TRANSLATIONS, [§21.3.26] SQL_LANGUAGES
// ---------------------------------------------------------------------------

fn assertions_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("CONSTRAINT", true).data("IS_DEFERRABLE", true).data("INITIALLY_DEFERRED", true).done()
}

/// [§21.3.19] Each assertion, deferrable or not and initially deferred or
/// not.
fn assertions(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let deferral = |a: &ConstraintDesc| vec![yes(a.attributes.deferrable), yes(a.attributes.initially_deferred)];
    seen.state.catalog.assertions().into_iter().map(|a| [ids(&catalog, &a.name).to_vec(), deferral(a)].concat()).collect()
}

fn translations_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TRANSLATION", true).ids("SOURCE_CHARACTER_SET", true).ids("TARGET_CHARACTER_SET", true).done()
}

fn languages_columns() -> Vec<ColumnDesc> {
    ["YEAR", "CONFORMANCE", "INTEGRITY", "IMPLEMENTATION", "BINDING_STYLE", "PROGRAMMING_LANGUAGE"]
        .iter()
        .fold(Columns::of().data("SQL_LANGUAGE_SOURCE", true), |c, part| c.data(&format!("SQL_LANGUAGE_{part}"), false))
        .done()
}

/// [§21.3.26] The conformance claimed: ISO 9075:1987, at Level 2, of the
/// binding style DIRECT, of no programming language; its integrity none,
/// of 1987 \[SQL_LANGUAGES_STANDARD_VALID_CHECK\].
fn languages(_: &Seen<'_>) -> Vec<Row> {
    vec![vec![text("ISO 9075"), text("1987"), text("2"), Value::Null, Value::Null, text("DIRECT"), Value::Null]]
}

// ---------------------------------------------------------------------------
// [§21.3.8] TABLES, [§21.3.9] VIEWS, [§21.3.10] COLUMNS
// ---------------------------------------------------------------------------

fn tables_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TABLE", true).data("TABLE_TYPE", true).done()
}

/// Each table, a BASE TABLE, a VIEW, or a GLOBAL or LOCAL TEMPORARY table
/// [§21.3.8]: a system table is a base table, as
/// INFORMATION_SCHEMA_CATALOG_NAME is \[§21.2.2\].
fn described_tables(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let kind = |o: &Object| match (o.view, seen.state.catalog.table(&o.name).and_then(|t| t.temporary).map(|t| t.kind)) {
        (true, _) => "VIEW",
        (false, None) => "BASE TABLE",
        (false, Some(TemporaryKind::Global)) => "GLOBAL TEMPORARY",
        (false, Some(TemporaryKind::CreatedLocal | TemporaryKind::Declared)) => "LOCAL TEMPORARY",
    };
    let row = |o: Object| [ids(&catalog, &o.name).to_vec(), vec![text(kind(&o))]].concat();
    Ok(objects(seen)?.into_iter().map(row).collect())
}

fn views_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TABLE", true).data("VIEW_DEFINITION", false).data("CHECK_OPTION", true).data("IS_UPDATABLE", true).done()
}

/// Each view, its query as written; WITH CHECK OPTION of its `<levels
/// clause>`, CASCADED when none is written [§11.19 SR13];
/// updatable or not [X3.135-1986 §6.5 SR3]. A view of the Information
/// Schema is not updatable \[§21.1\].
fn views(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows: Vec<Row> = seen
        .state
        .catalog
        .views()
        .into_iter()
        .map(|v| {
            let check = text(match v.check_option {
                Some(CheckLevels::Cascaded) => "CASCADED",
                Some(CheckLevels::Local) => "LOCAL",
                None => "NONE",
            });
            [ids(&catalog, &v.name).to_vec(), vec![long(&v.query), check, yes(v.updates.is_some())]].concat()
        })
        .collect();
    for v in super::information::views() {
        rows.push([ids(&catalog, &information(v.name)).to_vec(), vec![long(v.query), text("NONE"), yes(false)]].concat());
    }
    rows
}

fn columns_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE", true)
        .id("COLUMN_NAME", true)
        .number("ORDINAL_POSITION", true)
        .ids("DOMAIN", false)
        .data("COLUMN_DEFAULT", false)
        .data("IS_NULLABLE", false)
        .done()
}

/// Each column of each table, at its position, from 1, with its domain;
/// its own default as its `<default option>` would be written [§11.5],
/// none if it has its domain's alone; nullable unless constrained NOT
/// NULL.
fn columns(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for o in objects(seen)? {
        for (at, c) in o.columns.iter().enumerate() {
            let null = || vec![Value::Null, Value::Null, Value::Null];
            let domain = c.domain.as_ref().map_or_else(null, |d| ids(&catalog, &d.name).to_vec());
            let row = [vec![text(&c.name), number(at.saturating_add(1))], domain, vec![default_text(c.default.as_ref()), yes(!c.not_null)]]
                .concat();
            rows.push([ids(&catalog, &o.name).to_vec(), row].concat());
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// [§21.3.11] VIEW_TABLE_USAGE, [§21.3.12] VIEW_COLUMN_USAGE
// ---------------------------------------------------------------------------

fn view_table_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("VIEW", true).ids("TABLE", true).done()
}

/// Each table each view of the catalog names, in subqueries too, once.
/// The Information Schema's views read the Definition Schema, which "cannot
/// be accessed" \[§21.1\], so they are not among them.
fn view_tables(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for v in seen.state.catalog.views() {
        let mut named: Vec<&QualifiedName> = Vec::new();
        for t in &v.reads {
            if !named.contains(&t) {
                named.push(t);
            }
        }
        rows.extend(named.into_iter().map(|t| [ids(&catalog, &v.name), ids(&catalog, t)].concat()));
    }
    rows
}

fn view_column_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("VIEW", true).ids("TABLE", true).id("COLUMN_NAME", true).done()
}

/// Each column of a table or view each view of the catalog references,
/// explicitly or by `*`, once, as its query binds now.
///
/// # Errors
///
/// A view's query does not bind.
fn view_columns(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for v in seen.state.catalog.views() {
        for (t, c) in view_uses(seen.state, v)? {
            rows.push([ids(&catalog, &v.name).to_vec(), ids(&catalog, &t).to_vec(), vec![text(&c)]].concat());
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// [§21.3.23] CHARACTER_SETS, [§21.3.24] COLLATIONS
// ---------------------------------------------------------------------------

fn character_sets_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("CHARACTER_SET", true)
        .id("FORM_OF_USE", false)
        .number("NUMBER_OF_CHARACTERS", false)
        .ids("DEFAULT_COLLATE", true)
        .done()
}

/// [§21.3.23 Description 5] SQL_TEXT, its default collation SQL_TEXT.
fn character_sets(seen: &Seen<'_>) -> Vec<Row> {
    let (catalog, sql_text) = (catalog_name(seen), information(SQL_TEXT));
    let row = [ids(&catalog, &sql_text).to_vec(), vec![text(FORM_OF_USE), number(NUMBER_OF_CHARACTERS)], ids(&catalog, &sql_text).to_vec()];
    vec![row.concat()]
}

fn collations_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("COLLATION", true).ids("CHARACTER_SET", true).data("PAD_ATTRIBUTE", false).done()
}

/// [§21.3.24 Description 4] SQL_TEXT, of SQL_TEXT, its pad attribute,
/// implementation-defined, PAD SPACE: the shorter of two strings compared
/// is padded with spaces [X3.135-1986 §5.11 GR5].
fn collations(seen: &Seen<'_>) -> Vec<Row> {
    let (catalog, sql_text) = (catalog_name(seen), information(SQL_TEXT));
    vec![[ids(&catalog, &sql_text).to_vec(), ids(&catalog, &sql_text).to_vec(), vec![text("PAD SPACE")]].concat()]
}

/// [ISO/IEC 9075:1992 §10.4 SR2, §11.30 SR4] The usage privileges on the
/// character set and collation: the character set's PUBLIC's WITH GRANT
/// OPTION, the collation's PUBLIC's, both of the grantor "_SYSTEM".
pub(super) fn usage() -> [(&'static str, bool); 2] {
    [("CHARACTER SET", true), ("COLLATION", false)]
}

/// The name of the character set and collation.
pub(super) fn sql_text() -> QualifiedName {
    information(SQL_TEXT)
}
