//! The catalog: descriptions of schemas, tables, views, columns, indexes
//! and privileges.
//!
//! [X3.135-1986 §4.6] a `<schema>` is a "persistent object" created by implementor-defined
//! mechanisms, so where descriptions live is up to us. The `Catalog` trait is
//! the extension point; `MemoryCatalog` is the simplest implementation. A
//! persistent catalog would implement the same trait over files, or over
//! ordinary tables in one of the storage engines, as PostgreSQL does.

use std::collections::HashMap;
use std::fmt;

pub use crate::ast::{CheckLevels, Temporary, TemporaryKind};
use crate::value::{DataType, Value};

pub(crate) mod codec;
mod constraints;
mod domains;
mod privileges;

pub use constraints::{ConstraintAttributes, ConstraintDesc, ConstraintKind, ForeignKey, MatchType, ReferentialAction};
pub use domains::{DomainDesc, UsageDesc};
pub use privileges::{Actions, Privilege, PrivilegeDesc, applicable, merged};

/// A `<table name>` with its `<authorization identifier>` made explicit.
/// [X3.135-1986 §5.4 SR3] two table names are equal iff both parts are equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedName {
    /// The `<authorization identifier>` of the schema the table is in.
    pub auth: String,
    /// The table's `<identifier>` within that schema.
    pub ident: String,
}

impl QualifiedName {
    /// Its schema's name and its own, as a condition names it
    /// (`error::Subject`).
    pub fn parts(&self) -> (String, String) {
        (self.auth.clone(), self.ident.clone())
    }

    /// \[DESIGN\] The name of the schema `auth` itself, which its lock is
    /// taken by: no table's or view's, as no identifier is empty
    /// [ISO/IEC 9075:1992 §5.2].
    pub fn of_schema(auth: &str) -> QualifiedName {
        QualifiedName { auth: auth.to_owned(), ident: String::new() }
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ident.is_empty() {
            return write!(f, "schema {}", self.auth);
        }
        write!(f, "{}.{}", self.auth, self.ident)
    }
}

/// [X3.135-1986 §4.3] a column's description is its name, data type, and nullability.
#[derive(Debug, Clone)]
pub struct ColumnDesc {
    /// The `<column name>`.
    pub name: String,
    /// The column's `<data type>`.
    pub data_type: DataType,
    /// Whether it holds no nulls: a NOT NULL constraint is on it, or it is
    /// in the PRIMARY KEY (`TableDesc::refresh_not_null`).
    pub not_null: bool,
    /// [ISO/IEC 9075:1992 §11.5 GR1] The default value it includes, if its
    /// definition has a `<default clause>`.
    pub default: Option<ColumnDefault>,
    /// [ISO/IEC 9075:1992 §11.4 GR5] The domain it is of, if its definition
    /// names one, with that domain's default, kept with it as the domain
    /// changes (`db/domains.rs`).
    pub domain: Option<ColumnDomain>,
}

/// [ISO/IEC 9075:1992 §11.4 GR5] The domain a column is of.
#[derive(Debug, Clone)]
pub struct ColumnDomain {
    /// The `<domain name>`.
    pub name: QualifiedName,
    /// The domain's default, which is the column's if it has none of its
    /// own [§11.5 GR2b].
    pub default: Option<ColumnDefault>,
}

/// [ISO/IEC 9075:1992 §11.5 GR1] The default value of a column descriptor,
/// derived from a `<default option>`.
#[derive(Debug, Clone)]
pub enum ColumnDefault {
    /// That of a `<literal>`, or the null value \[GR1a, b\].
    Value(Value),
    /// CURRENT_USER, SESSION_USER or SYSTEM_USER, whichever it is when the
    /// default is taken \[GR1c\]: in this implementation, the authorization
    /// identifier the statement runs under, each.
    User,
}

impl ColumnDesc {
    /// [ISO/IEC 9075:1992 §11.5 GR2] Its default value, for a statement run
    /// under the authorization identifier `auth`: before store assignment
    /// pads a fixed-length string \[GR1b iii, GR1c ii\].
    pub fn default_value(&self, auth: &str) -> Value {
        // [GR2a] its own; [GR2b] its domain's; [GR2c] null.
        let of_domain = self.domain.as_ref().and_then(|d| d.default.as_ref());
        match self.default.as_ref().or(of_domain) {
            Some(ColumnDefault::Value(v)) => v.clone(),
            Some(ColumnDefault::User) => Value::Char(auth.to_string()),
            None => Value::Null,
        }
    }
}

/// [X3.135-1986 §4.4] a base table's description, plus which storage engine holds its rows.
#[derive(Debug, Clone)]
pub struct TableDesc {
    /// The table's name.
    pub name: QualifiedName,
    /// The columns in order: a column's position is its index here.
    pub columns: Vec<ColumnDesc>,
    /// Its constraints [ISO/IEC 9075:1992 §11.6]: SQL-86's NOT NULL and
    /// UNIQUE [X3.135-1986 §6.3, §6.4] among them. Each column's `not_null`
    /// is kept in step with them (`TableDesc::refresh_not_null`).
    pub constraints: Vec<ConstraintDesc>,
    /// \[DESIGN\] The name of the storage engine that holds the rows.
    pub engine: String,
    /// \[EXTENSION\] The engine's table options it sets, by name, in order
    /// of name; the others are their defaults (RULES.md D41).
    pub options: Vec<(String, Value)>,
    /// \[DESIGN\] Which storage of the table holds its rows: 0 for the
    /// first, one more for each ALTER TABLE that gave it a new one
    /// (`db/ddl.rs`). Row changes are logged with it, so recovery applies
    /// each to the storage it was made in.
    pub generation: u32,
    /// [ISO/IEC 9075:1992 §4.9, §11.3] Of a temporary table, which kind,
    /// and what a commit does to its rows; none for a persistent base
    /// table.
    pub temporary: Option<Temporary>,
}

impl TableDesc {
    /// The position of the column named `name`, if the table has one.
    pub fn column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// The position and description of the column named `name`, if any.
    pub fn column_desc(&self, name: &str) -> Option<(usize, &ColumnDesc)> {
        self.columns.iter().enumerate().find(|(_, c)| c.name == name)
    }

    /// The types of the columns at `positions`, if the table has them all:
    /// those of an index's key.
    pub fn types(&self, positions: &[usize]) -> Option<Vec<DataType>> {
        positions.iter().map(|&p| self.columns.get(p).map(|c| c.data_type)).collect()
    }
}

/// [ISO/IEC 9075:1992 §11.11 GR2, note] The columns a view's query or a
/// CHECK's condition sees of each base table it reads: as many as the table
/// had when it was defined. A column added to a table is its last [§11.11
/// GR4], so those are the first of them; the query's implicit column
/// references (*, `<qualifier>`.*, TABLE, a NATURAL join's, CORRESPONDING's,
/// an unqualified name's table) "are replaced by explicit `<column
/// reference>`s when the clause is originally evaluated", and a column added
/// changes none. A table it does not list it sees whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Horizon(pub Vec<(QualifiedName, usize)>);

/// No horizon at all: every table seen whole.
pub static WHOLE: Horizon = Horizon(Vec::new());

impl Horizon {
    /// The degree of each base table of `tables` in `catalog` now.
    pub fn of(catalog: &dyn Catalog, tables: &[QualifiedName]) -> Horizon {
        Horizon(tables.iter().filter_map(|t| catalog.table(t).map(|d| (t.clone(), d.columns.len()))).collect())
    }

    /// How many columns of `table` are seen, if not all.
    pub fn width(&self, table: &QualifiedName) -> Option<usize> {
        self.0.iter().find(|(t, _)| t == table).map(|(_, w)| *w)
    }

    /// It once the column at `at` of `table` is dropped [§11.15 GR4]: one
    /// fewer seen, if the column was.
    #[must_use]
    pub fn without(&self, table: &QualifiedName, at: usize) -> Horizon {
        Horizon(self.0.iter().map(|(t, w)| (t.clone(), if t == table && at < *w { w.saturating_sub(1) } else { *w })).collect())
    }
}

/// [X3.135-1986 §4.4, §6.5] A viewed table's description.
#[derive(Debug, Clone)]
pub struct ViewDesc {
    /// The view's name.
    pub name: QualifiedName,
    /// Its columns [X3.135-1986 §6.5 SR7].
    pub columns: Vec<ColumnDesc>,
    /// Its `<query specification>` as written, in which a `<table name>`
    /// without an `<authorization identifier>` has the view's
    /// [X3.135-1986 §5.4 SR2].
    pub query: String,
    /// WITH CHECK OPTION [X3.135-1986 §6.5 GR3], of its levels [ISO/IEC
    /// 9075:1992 §11.19 SR13], if written.
    pub check_option: Option<CheckLevels>,
    /// Whether it is a grouped view [X3.135-1986 §6.5 SR8].
    pub grouped: bool,
    /// For an updatable view [X3.135-1986 §6.5 SR3], the table its `<from
    /// clause>` names, and the column of that table each of its columns is
    /// [X3.135-1986 §6.5 GR2]; none for a read-only view.
    pub updates: Option<(QualifiedName, Vec<usize>)>,
    /// The tables its `<query specification>` names, in subqueries too
    /// [X3.135-1986 §6.6 SR3b i 2].
    pub reads: Vec<QualifiedName>,
    /// The columns of each table it reads its query sees.
    pub horizon: Horizon,
    /// What its columns are drawn from, if that is known: what its
    /// owner's REFERENCES on it are found from [ISO/IEC 9075:1992 §11.19
    /// GR6].
    pub underlying: Option<Underlying>,
}

/// [ISO/IEC 9075:1992 §11.19 GR6] What a view's columns are drawn from:
/// the leaf underlying tables of its query, base tables each; and, of each
/// of its columns, the columns of them it is drawn from, each by its table
/// and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Underlying {
    /// The leaf underlying tables.
    pub leaves: Vec<QualifiedName>,
    /// Of each column of the view, the columns it is drawn from.
    pub columns: Vec<Vec<(QualifiedName, String)>>,
}

/// \[EXTENSION\] An index. X3.135-1986 has no CREATE INDEX; an implementation
/// may add facilities [X3.135-1986 §3.4], and here they are created through the Rust API or
/// automatically to enforce UNIQUE.
#[derive(Debug, Clone)]
pub struct IndexDesc {
    /// The index's name, unique in the catalog.
    pub name: String,
    /// The table indexed.
    pub table: QualifiedName,
    /// The positions, in the table, of the indexed columns.
    pub columns: Vec<usize>,
    /// The name of the `IndexMethod` that builds and searches the index.
    pub method: String,
    /// Whether the index enforces a UNIQUE constraint.
    pub unique: bool,
    /// \[EXTENSION\] The method's index options it sets, by name, in order
    /// of name; the others are their defaults (RULES.md D41).
    pub options: Vec<(String, Value)>,
}

/// \[DESIGN\] Where schemas, tables and indexes are described.
///
/// Contract (RULES.md R-SAFE-3; checked for `MemoryCatalog` by
/// `test/integration/contracts.rs`):
/// * Callers create a schema only if `schema_exists` is false, and add an index
///   only if no index has its name and its table exists.
/// * After `create_schema`, `schema_exists` is true, `schemas` lists it
///   after those created before it, `schema_owner` returns its owner, and
///   `table` returns each of its tables; `tables` returns every table in a stable order. Callers
///   remove a schema only once its tables and views are removed; after
///   `remove_schema`, `schema_exists` is false, and `schemas` lists the
///   others as before.
/// * After `add_index`, `index` returns it by name, and `indexes` returns it
///   among exactly the indexes of its table; after `remove_index`, neither
///   does.
/// * Callers add a view only if no table or view has its name. After
///   `add_views`, `view` returns each of them; `table` and `tables` do not.
/// * Callers grant privileges only on tables and views that exist. After `grant`,
///   `privileges` returns each of them among exactly those of its table, in
///   the order granted.
/// * After `remove_table`, `table` and `tables` do not return it; after
///   `remove_view`, `view` and `views` do not; after `revoke_all`,
///   `privileges` returns none on that table or view. `views` returns
///   every view in a stable order.
/// * After `put_domain`, `domain` returns it by name, and `domains` among
///   every domain in a stable order; after `remove_domain`, neither does.
///   So too `put_assertion`, `assertion`, `assertions` and
///   `remove_assertion`.
/// * `duplicate` returns an independent copy: changing either leaves the
///   other as it was. Each snapshot of the database's definitions holds its
///   own copy, and a change of definitions works on a fresh duplicate
///   (RULES.md D27). A catalog may make this cheap by sharing what is
///   unchanged.
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait Catalog: Send + Sync {
    /// Whether a schema of this `<schema name>` exists.
    fn schema_exists(&self, schema: &str) -> bool;
    /// The name of every schema, in the order created: those
    /// `schema_exists` is true of.
    fn schemas(&self) -> Vec<&str>;
    /// The `<authorization identifier>` that owns the schema `schema`
    /// [ISO/IEC 9075:1992 §11.1 GR2], if it exists.
    fn schema_owner(&self, schema: &str) -> Option<&str>;
    /// Record a schema of the name `schema`, owned by `owner`, and all its
    /// tables in one step [X3.135-1986 §6.1; ISO/IEC 9075:1992 §11.1].
    fn create_schema(&mut self, schema: &str, owner: &str, tables: Vec<TableDesc>);
    /// Forget the schema `schema`, which holds no table or view: one
    /// dropped [ISO/IEC 9075:1992 §11.2 GR8].
    fn remove_schema(&mut self, schema: &str);
    /// Record a base table of a schema that exists, defined on its own
    /// [ISO/IEC 9075:1992 §11.3 GR3]. After it, `table` returns `desc`.
    fn add_table(&mut self, desc: TableDesc);
    /// The table with this name, if any.
    fn table(&self, name: &QualifiedName) -> Option<&TableDesc>;
    /// Record an index.
    fn add_index(&mut self, index: IndexDesc);
    /// Forget the index with this name: one dropped, or whose build failed.
    fn remove_index(&mut self, name: &str);
    /// The index with this name, if any.
    fn index(&self, name: &str) -> Option<&IndexDesc>;
    /// The indexes of `table`.
    fn indexes(&self, table: &QualifiedName) -> Vec<&IndexDesc>;
    /// Every table, in a stable order, so a `Database` can give each its storage.
    fn tables(&self) -> Vec<&TableDesc>;
    /// Record the views of a `<schema>` [X3.135-1986 §6.5].
    fn add_views(&mut self, views: Vec<ViewDesc>);
    /// The view with this name, if any.
    fn view(&self, name: &QualifiedName) -> Option<&ViewDesc>;
    /// Record the privileges a `<schema>` grants [X3.135-1986 §6.6].
    fn grant(&mut self, privileges: Vec<PrivilegeDesc>);
    /// The privileges granted on `table`, in the order granted.
    fn privileges(&self, table: &QualifiedName) -> Vec<&PrivilegeDesc>;
    /// Every view, in a stable order.
    fn views(&self) -> Vec<&ViewDesc>;
    /// Replace the descriptor of the base table `desc` names, which exists:
    /// one altered [ISO/IEC 9075:1992 §11.10 GR1]. After it, `table`
    /// returns `desc`.
    fn replace_table(&mut self, desc: TableDesc);
    /// Forget the base table with this name: one dropped
    /// [ISO/IEC 9075:1992 §11.18 GR2]. Its indexes, the views over it and
    /// the privileges on it are removed by the caller.
    fn remove_table(&mut self, name: &QualifiedName);
    /// Forget the view with this name: one dropped with what it reads.
    fn remove_view(&mut self, name: &QualifiedName);
    /// Forget every privilege granted on `table`, a table or a view
    /// [ISO/IEC 9075:1992 §11.18 GR1].
    fn revoke_all(&mut self, table: &QualifiedName);
    /// Make `privileges` every privilege granted on `table`, in place of
    /// those it had: what a `<grant statement>` or a `<revoke statement>`
    /// left [ISO/IEC 9075:1992 §11.36, §11.37].
    fn set_privileges(&mut self, table: &QualifiedName, privileges: Vec<PrivilegeDesc>);
    /// Record `domain`, in place of any of its name [ISO/IEC 9075:1992
    /// §11.21, §11.22]. Callers add one only to a schema that exists.
    fn put_domain(&mut self, domain: DomainDesc);
    /// The domain with this name, if any.
    fn domain(&self, name: &QualifiedName) -> Option<&DomainDesc>;
    /// Every domain, in a stable order.
    fn domains(&self) -> Vec<&DomainDesc>;
    /// Forget the domain with this name: one dropped [§11.27 GR3].
    fn remove_domain(&mut self, name: &QualifiedName);
    /// Record the assertion `assertion`, a CHECK of no table, in place of
    /// any of its name [ISO/IEC 9075:1992 §11.34].
    fn put_assertion(&mut self, assertion: ConstraintDesc);
    /// The assertion with this name, if any.
    fn assertion(&self, name: &QualifiedName) -> Option<&ConstraintDesc>;
    /// Every assertion, in a stable order.
    fn assertions(&self) -> Vec<&ConstraintDesc>;
    /// Forget the assertion with this name: one dropped [§11.35 GR1].
    fn remove_assertion(&mut self, name: &QualifiedName);
    /// An independent copy of this catalog.
    fn duplicate(&self) -> Box<dyn Catalog>;
}

/// The `<authorization identifier>` that owns the schema `schema`
/// [ISO/IEC 9075:1992 §11.1 GR2], as `catalog` keeps it; for one it does
/// not keep, a system table's or none, an identifier of its own name, as
/// every schema's owner was in SQL-86 [X3.135-1986 §4.15].
pub fn owner_of<'a>(catalog: &'a dyn Catalog, schema: &'a str) -> &'a str {
    catalog.schema_owner(schema).unwrap_or(schema)
}

/// \[DESIGN\] The built-in catalog, held in memory; `Database::new` uses it.
#[derive(Default, Clone)]
pub struct MemoryCatalog {
    /// Each schema's name, and its owner, in the order created.
    schemas: Vec<(String, String)>,
    tables: HashMap<QualifiedName, TableDesc>,
    indexes: Vec<IndexDesc>,
    views: HashMap<QualifiedName, ViewDesc>,
    privileges: Vec<PrivilegeDesc>,
    domains: HashMap<QualifiedName, DomainDesc>,
    assertions: HashMap<QualifiedName, ConstraintDesc>,
}

impl Catalog for MemoryCatalog {
    fn duplicate(&self) -> Box<dyn Catalog> {
        Box::new(self.clone())
    }
    fn schema_exists(&self, schema: &str) -> bool {
        self.schemas.iter().any(|(s, _)| s == schema)
    }
    fn schemas(&self) -> Vec<&str> {
        self.schemas.iter().map(|(s, _)| s.as_str()).collect()
    }
    fn schema_owner(&self, schema: &str) -> Option<&str> {
        self.schemas.iter().find(|(s, _)| s == schema).map(|(_, owner)| owner.as_str())
    }
    fn create_schema(&mut self, schema: &str, owner: &str, tables: Vec<TableDesc>) {
        self.schemas.push((schema.to_owned(), owner.to_owned()));
        for t in tables {
            self.tables.insert(t.name.clone(), t);
        }
    }
    fn remove_schema(&mut self, schema: &str) {
        self.schemas.retain(|(s, _)| s != schema);
    }
    fn add_table(&mut self, desc: TableDesc) {
        self.tables.insert(desc.name.clone(), desc);
    }
    fn table(&self, name: &QualifiedName) -> Option<&TableDesc> {
        self.tables.get(name)
    }
    fn add_index(&mut self, index: IndexDesc) {
        self.indexes.push(index);
    }
    fn index(&self, name: &str) -> Option<&IndexDesc> {
        self.indexes.iter().find(|i| i.name == name)
    }
    fn remove_index(&mut self, name: &str) {
        self.indexes.retain(|i| i.name != name);
    }
    fn indexes(&self, table: &QualifiedName) -> Vec<&IndexDesc> {
        self.indexes.iter().filter(|i| &i.table == table).collect()
    }
    fn tables(&self) -> Vec<&TableDesc> {
        let mut tables: Vec<&TableDesc> = self.tables.values().collect();
        tables.sort_by_key(|t| t.name.to_string());
        tables
    }
    fn add_views(&mut self, views: Vec<ViewDesc>) {
        for v in views {
            self.views.insert(v.name.clone(), v);
        }
    }
    fn view(&self, name: &QualifiedName) -> Option<&ViewDesc> {
        self.views.get(name)
    }
    fn grant(&mut self, privileges: Vec<PrivilegeDesc>) {
        self.privileges.extend(privileges);
    }
    fn privileges(&self, table: &QualifiedName) -> Vec<&PrivilegeDesc> {
        self.privileges.iter().filter(|p| &p.table == table).collect()
    }
    fn views(&self) -> Vec<&ViewDesc> {
        let mut views: Vec<&ViewDesc> = self.views.values().collect();
        views.sort_by_key(|v| v.name.to_string());
        views
    }
    fn replace_table(&mut self, desc: TableDesc) {
        self.tables.insert(desc.name.clone(), desc);
    }
    fn remove_table(&mut self, name: &QualifiedName) {
        self.tables.remove(name);
    }
    fn remove_view(&mut self, name: &QualifiedName) {
        self.views.remove(name);
    }
    fn revoke_all(&mut self, table: &QualifiedName) {
        self.privileges.retain(|p| &p.table != table);
    }
    fn set_privileges(&mut self, table: &QualifiedName, privileges: Vec<PrivilegeDesc>) {
        self.revoke_all(table);
        self.privileges.extend(privileges);
    }
    fn put_domain(&mut self, domain: DomainDesc) {
        self.domains.insert(domain.name.clone(), domain);
    }
    fn domain(&self, name: &QualifiedName) -> Option<&DomainDesc> {
        self.domains.get(name)
    }
    fn domains(&self) -> Vec<&DomainDesc> {
        let mut domains: Vec<&DomainDesc> = self.domains.values().collect();
        domains.sort_by_key(|d| d.name.to_string());
        domains
    }
    fn remove_domain(&mut self, name: &QualifiedName) {
        self.domains.remove(name);
    }
    fn put_assertion(&mut self, assertion: ConstraintDesc) {
        self.assertions.insert(assertion.name.clone(), assertion);
    }
    fn assertion(&self, name: &QualifiedName) -> Option<&ConstraintDesc> {
        self.assertions.get(name)
    }
    fn assertions(&self) -> Vec<&ConstraintDesc> {
        let mut assertions: Vec<&ConstraintDesc> = self.assertions.values().collect();
        assertions.sort_by_key(|a| a.name.to_string());
        assertions
    }
    fn remove_assertion(&mut self, name: &QualifiedName) {
        self.assertions.remove(name);
    }
}
