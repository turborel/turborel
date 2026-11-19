//! The catalog: descriptions of schemas, tables, views, columns,
//! constraints, domains, assertions, collations, sequence generators,
//! user-defined types, indexes, privileges and registered XML Schemas.
//!
//! [X3.135-1986 §4.6] a `<schema>` is a "persistent object" created by implementor-defined
//! mechanisms, so where descriptions live is up to us. The `Catalog` trait is
//! the extension point; `MemoryCatalog`, held in memory, is the one built
//! in, made durable by the log and a checkpoint's image of it
//! (`catalog/codec.rs`). Another catalog would implement the same trait
//! over files, or over ordinary tables in one of the storage engines, as
//! PostgreSQL does.

use std::fmt;

pub use crate::ast::{CheckLevels, Temporary, TemporaryKind};
use crate::value::{DataType, Value};

pub(crate) mod codec;
mod collations;
mod columns;
mod constraints;
mod domains;
mod embedding;
mod horizon;
mod key_expressions;
mod keys;
mod memory;
mod modules;
mod periods;
mod privileges;
mod roles;
mod routines;
mod sequences;
mod structured;
mod triggers;
mod user_types;
pub mod xml_schemas;
pub mod xquery_modules;

use crate::value::collation::CollationRef;
pub use collations::CollationDesc;
pub use columns::{ColumnDefault, ColumnDesc, ColumnDomain, Generated, Generation, Identity};
pub use constraints::{ConstraintAttributes, ConstraintDesc, ConstraintKind, ForeignKey, MatchType, ReferentialAction};
pub use domains::{DomainDesc, UsageDesc, usage_held};
pub use embedding::{Embeddings, ModelDesc, ModelOption, PipelineDesc, TRIGGER_SUFFIXES};
pub(crate) use embedding::{embeddings, put_embeddings};
pub use horizon::{Horizon, Indicated, WHOLE};
pub use key_expressions::{EXPRESSION_PART, KeyExpression};
pub use keys::KeyShape;
pub use memory::MemoryCatalog;
pub use modules::{ModuleDesc, module_of_schema, module_schema, routine_path};
pub use periods::PeriodDesc;
pub use privileges::{Actions, Privilege, PrivilegeDesc, SYSTEM_GRANTOR, TypedActions, applicable, merged};
pub use roles::{Authorization, Principal, RoleAuthorization, Roles, Who};
pub use routines::{
    DataAccess, DistanceDesc, GenericParameter, GenericTable, PTF_NAME_LENGTH, ParameterMode, PtfDesc, RoutineKind, SqlParameter,
    SqlRoutineDesc, Stamps, TableSemantics, ptf_name_type, ptf_status_type,
};
pub use sequences::SequenceDesc;
pub use structured::{
    AttributeDesc, CONSTRUCTOR_SUFFIX, CastDesc, MAX_HIERARCHY_DEPTH, MUTATOR_SUFFIX, MethodHost, MethodSpecDesc, OBSERVER_SUFFIX,
    OrderingCategory, OrderingDesc, RefRepresentation, StructuredDesc, TransformGroupDesc, TransformNames, TypedDesc, method_executable,
    method_host, proper_subtables, proper_supertables, type_methods, udt_qualified,
};
pub use triggers::{ActionTime, TransitionNames, TriggerDesc, TriggerEvent, TriggerLevel};
pub use user_types::{Routine, RoutineDesc, TO_DISTINCT_SUFFIX, TO_SOURCE_SUFFIX, UserTypeDesc, based_on, types_in};
pub use xml_schemas::XmlSchemaDesc;
pub use xquery_modules::XQueryModuleDesc;

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
    /// [ISO/IEC 9075-2:2023 §4.15.3] Of a typed table, its structured type,
    /// self-referencing column and supertable.
    pub typed: Option<TypedDesc>,
    /// [ISO/IEC 9075-2:2023 §4.16.1] Its period descriptors.
    pub periods: Vec<PeriodDesc>,
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
        positions.iter().map(|&p| self.columns.get(p).map(|c| c.data_type.clone())).collect()
    }
}

/// [X3.135-1986 §4.4, §6.5] A viewed table's description.
#[derive(Debug, Clone)]
pub struct ViewDesc {
    /// The view's name.
    pub name: QualifiedName,
    /// Its columns [X3.135-1986 §6.5 SR7].
    pub columns: Vec<ColumnDesc>,
    /// Its `<query specification>` as written.
    pub query: String,
    /// The schema of the names in `query` written without one [ISO/IEC
    /// 9075:1992 §5.4 SR8]: the view's own, in a `<schema definition>`;
    /// else the session's default schema as it was defined.
    pub names: String,
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
    /// [ISO/IEC 9075-2:2023 §4.16] Of a referenceable view, its structured
    /// type, self-referencing column and superview.
    pub typed: Option<TypedDesc>,
    /// What its columns are drawn from, if recorded: what its owner's
    /// REFERENCES on it are found from [ISO/IEC 9075:1992 §11.19 GR6].
    pub underlying: Option<Underlying>,
}

/// [ISO/IEC 9075:1992 §11.19 GR6] What a view's columns are drawn from:
/// the leaf underlying tables of its query, base tables and views [§4.9];
/// and, of each of its columns, its underlying columns of them, each by
/// its table and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Underlying {
    /// The leaf underlying tables.
    pub leaves: Vec<QualifiedName>,
    /// Of each column of the view, its underlying columns of the leaves.
    pub columns: Vec<Vec<(QualifiedName, String)>>,
}

/// \[EXTENSION\] An index. X3.135-1986 has no CREATE INDEX; an implementation
/// may add facilities [X3.135-1986 §3.4], and here they are created by CREATE INDEX
/// (RULES.md D34), through the Rust API, or automatically to enforce UNIQUE.
#[derive(Debug, Clone)]
pub struct IndexDesc {
    /// The index's name, unique in the catalog.
    pub name: String,
    /// The table indexed.
    pub table: QualifiedName,
    /// The positions, in the table, of the indexed columns: of each key
    /// part, its column's, or `EXPRESSION_PART` of one that is an
    /// expression (`expressions`).
    pub columns: Vec<usize>,
    /// The name of the `IndexMethod` that builds and searches the index.
    pub method: String,
    /// Whether the index enforces a UNIQUE constraint.
    pub unique: bool,
    /// \[EXTENSION\] The method's index options it sets, by name, in order
    /// of name; the others are their defaults (RULES.md D41).
    pub options: Vec<(String, Value)>,
    /// \[EXTENSION\] The collation of each key column if any is not the
    /// default, its key that column's sort key; empty if none is, so a key
    /// is the columns' values (`catalog/keys.rs`, RULES.md D60). Derived
    /// from the table's columns, and not stored.
    pub collations: Vec<Option<CollationRef>>,
    /// [ISO/IEC 9075-2:2023 §11.7] Whether it holds the keys with a null
    /// too: the index of a UNIQUE NULLS NOT DISTINCT constraint, whose
    /// nulls its keys compare as values, or one built again to take its
    /// place. Every other index holds none, as no comparison with a null
    /// is true [X3.135-1986 §5.11 GR2]. Derived from the table's
    /// constraints, and not stored (`catalog/keys.rs`).
    pub nulls: bool,
    /// \[EXTENSION\] Its key parts that are expressions, in key order
    /// (`catalog/key_expressions.rs`, RULES.md D92).
    pub expressions: Vec<KeyExpression>,
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
///   So too `put_collation`, `collation`, `collations` and
///   `remove_collation`; `put_sequence`, `sequence`, `sequences` and
///   `remove_sequence`; and `put_user_type`, `user_type`, `user_types` and
///   `remove_user_type`.
///   So too `put_xml_schema`, `xml_schema`, `xml_schemas` and
///   `remove_xml_schema`; and `put_xquery_module`, `xquery_module`,
///   `xquery_modules` and `remove_xquery_module`.
///   So too `put_assertion`, `assertion`, `assertions` and
///   `remove_assertion`; and `put_trigger`, `trigger`, `triggers` and
///   `remove_trigger`, `triggers` and `triggers_on` in the order of
///   their timestamps of creation; and `put_routine`, `routine`,
///   `routines`, `routines_named` and `remove_routine`, by specific
///   name, `routines` and `routines_named` in the order of their
///   timestamps of creation.
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
    /// Every temporary table, in no fixed order, so a session can stand
    /// its own instances in for them at each statement without the
    /// ordering `tables` does (`db/temporary.rs`). By default, those among
    /// `tables`; a catalog that can give them unordered should.
    fn temporary_tables(&self) -> Vec<&TableDesc> {
        self.tables().into_iter().filter(|t| t.temporary.is_some()).collect()
    }
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
    /// Record a collation, in place of one of its name.
    fn put_collation(&mut self, collation: CollationDesc);
    /// The collation of this name a `<collation definition>` made, if any.
    fn collation(&self, name: &QualifiedName) -> Option<&CollationDesc>;
    /// Every collation definitions made, in a stable order.
    fn collations(&self) -> Vec<&CollationDesc>;
    /// Forget the collation of this name.
    fn remove_collation(&mut self, name: &QualifiedName);
    /// Record a sequence generator, in place of one of its name.
    fn put_sequence(&mut self, sequence: SequenceDesc);
    /// The sequence generator of this name, if any.
    fn sequence(&self, name: &QualifiedName) -> Option<&SequenceDesc>;
    /// Every sequence generator, in a stable order.
    fn sequences(&self) -> Vec<&SequenceDesc>;
    /// Forget the sequence generator of this name.
    fn remove_sequence(&mut self, name: &QualifiedName);
    /// Record a user-defined type, in place of one of its name [ISO/IEC
    /// 9075-2:2023 §11.51 GR1].
    fn put_user_type(&mut self, user_type: UserTypeDesc);
    /// The user-defined type of this name, if any.
    fn user_type(&self, name: &QualifiedName) -> Option<&UserTypeDesc>;
    /// Every user-defined type, in a stable order.
    fn user_types(&self) -> Vec<&UserTypeDesc>;
    /// Forget the user-defined type of this name: one dropped [§11.59 GR].
    fn remove_user_type(&mut self, name: &QualifiedName);
    /// Record a structured type, in place of one of its name [ISO/IEC
    /// 9075-2:2023 §11.51 GR1].
    fn put_structured_type(&mut self, structured: StructuredDesc);
    /// The structured type of this name, if any.
    fn structured_type(&self, name: &QualifiedName) -> Option<&StructuredDesc>;
    /// Every structured type, in a stable order.
    fn structured_types(&self) -> Vec<&StructuredDesc>;
    /// Forget the structured type of this name.
    fn remove_structured_type(&mut self, name: &QualifiedName);
    /// Record a user-defined cast, in place of one of its source and target
    /// types [ISO/IEC 9075-2:2023 §11.63 GR1].
    fn put_cast(&mut self, cast: CastDesc);
    /// Every user-defined cast, in a stable order.
    fn casts(&self) -> Vec<&CastDesc>;
    /// Forget the user-defined cast from `source` to `target`.
    fn remove_cast(&mut self, source: &DataType, target: &DataType);
    /// Record a user-defined ordering, in place of its type's [§11.65 GR].
    fn put_ordering(&mut self, ordering: OrderingDesc);
    /// The ordering of the user-defined type of this name, if any.
    fn ordering(&self, udt: &QualifiedName) -> Option<&OrderingDesc>;
    /// Every user-defined ordering, in a stable order.
    fn orderings(&self) -> Vec<&OrderingDesc>;
    /// Forget the ordering of the type of this name.
    fn remove_ordering(&mut self, udt: &QualifiedName);
    /// Record the transform groups of the type of this name, in place of
    /// those it has [§11.67 GR]; none forgets them.
    fn put_transforms(&mut self, udt: &QualifiedName, groups: Vec<TransformGroupDesc>);
    /// The transform groups of the type of this name: none if it has none.
    fn transforms(&self, udt: &QualifiedName) -> &[TransformGroupDesc];
    /// Every type with transform groups, and its groups, in a stable order.
    fn all_transforms(&self) -> Vec<(&QualifiedName, &[TransformGroupDesc])>;
    /// Forget the domain with this name: one dropped [§11.27 GR3].
    fn remove_domain(&mut self, name: &QualifiedName);
    /// Record a registered XML Schema, in place of one of its name
    /// [ISO/IEC 9075-14:2006 draft §4.2.6].
    fn put_xml_schema(&mut self, xml_schema: XmlSchemaDesc);
    /// The registered XML Schema of this name, if any.
    fn xml_schema(&self, name: &QualifiedName) -> Option<&XmlSchemaDesc>;
    /// Every registered XML Schema, in a stable order.
    fn xml_schemas(&self) -> Vec<&XmlSchemaDesc>;
    /// Forget the registered XML Schema of this name.
    fn remove_xml_schema(&mut self, name: &QualifiedName);
    /// \[EXTENSION\] Record a registered XQuery module, in place of one of
    /// its name.
    fn put_xquery_module(&mut self, module: XQueryModuleDesc);
    /// The registered XQuery module of this name, if any.
    fn xquery_module(&self, name: &QualifiedName) -> Option<&XQueryModuleDesc>;
    /// Every registered XQuery module, in a stable order.
    fn xquery_modules(&self) -> Vec<&XQueryModuleDesc>;
    /// Forget the registered XQuery module of this name.
    fn remove_xquery_module(&mut self, name: &QualifiedName);
    /// Record the assertion `assertion`, a CHECK of no table, in place of
    /// any of its name [ISO/IEC 9075:1992 §11.34].
    fn put_assertion(&mut self, assertion: ConstraintDesc);
    /// The assertion with this name, if any.
    fn assertion(&self, name: &QualifiedName) -> Option<&ConstraintDesc>;
    /// Every assertion, in a stable order.
    fn assertions(&self) -> Vec<&ConstraintDesc>;
    /// Forget the assertion with this name: one dropped [§11.35 GR1].
    fn remove_assertion(&mut self, name: &QualifiedName);
    /// Record the trigger `trigger`, in place of any of its name [ISO/IEC
    /// 9075-2:2023 §11.49 GR5].
    fn put_trigger(&mut self, trigger: TriggerDesc);
    /// The trigger with this name, if any.
    fn trigger(&self, name: &QualifiedName) -> Option<&TriggerDesc>;
    /// [ISO/IEC 9075-2:2023 §4.46] Whether it has a trigger: none, so a
    /// statement keeps no state changes.
    fn any_triggers(&self) -> bool {
        !self.triggers().is_empty()
    }
    /// Every trigger, in the order of their timestamps of creation.
    fn triggers(&self) -> Vec<&TriggerDesc>;
    /// The triggers whose subject table is `table`, in the order of their
    /// timestamps of creation [§4.46.1]: by default, those among
    /// `triggers`.
    fn triggers_on(&self, table: &QualifiedName) -> Vec<&TriggerDesc> {
        self.triggers().into_iter().filter(|t| t.table == *table).collect()
    }
    /// Forget the trigger with this name: one dropped [§11.50 GR3].
    fn remove_trigger(&mut self, name: &QualifiedName);
    /// Record the SQL-invoked routine `routine`, in place of any of its
    /// specific name [ISO/IEC 9075-2:2023 §11.60 GR3].
    fn put_routine(&mut self, routine: SqlRoutineDesc);
    /// The SQL-invoked routine of this specific name, if any.
    fn routine(&self, specific: &QualifiedName) -> Option<&SqlRoutineDesc>;
    /// Whether it has an SQL-invoked routine: none, so a routine
    /// invocation costs the lookup of none.
    fn any_routines(&self) -> bool {
        !self.routines().is_empty()
    }
    /// Every SQL-invoked routine, in the order of their timestamps of
    /// creation.
    fn routines(&self) -> Vec<&SqlRoutineDesc>;
    /// The SQL-invoked routines of the `<schema qualified routine name>`
    /// `name`, in the order of `routines`.
    fn routines_named(&self, name: &QualifiedName) -> Vec<&SqlRoutineDesc> {
        self.routines().into_iter().filter(|r| r.name == *name).collect()
    }
    /// Forget the SQL-invoked routine of this specific name: one dropped
    /// [§11.62 GR2].
    fn remove_routine(&mut self, specific: &QualifiedName);
    /// Record the SQL-server module `module`, in place of any of its name
    /// [ISO/IEC 9075-4:2006 draft §9.21 GR3].
    fn put_module(&mut self, module: ModuleDesc);
    /// The SQL-server module of this name, if any.
    fn module(&self, name: &QualifiedName) -> Option<&ModuleDesc>;
    /// Every SQL-server module, in the order of their timestamps of
    /// creation.
    fn modules(&self) -> Vec<&ModuleDesc>;
    /// Forget the SQL-server module of this name: one dropped [§9.22 GR2].
    /// Its routines and tables are removed by the caller.
    fn remove_module(&mut self, name: &QualifiedName);
    /// [ISO/IEC 9075-2:2023 §4.42.3] The roles and role authorizations.
    fn roles(&self) -> &Roles;
    /// Make `roles` the roles and role authorizations, in place of those
    /// there were: what a `<role definition>`, a `<grant role statement>`,
    /// a `<revoke role statement>` or a `<drop role statement>` left
    /// [§12.4 to §12.7].
    fn set_roles(&mut self, roles: Roles);
    /// \[EXTENSION\] The embedding models and vector pipelines (the vector
    /// design's §12.1, §12.5).
    fn embeddings(&self) -> &Embeddings;
    /// Make `embeddings` the embedding models and vector pipelines, in
    /// place of those there were.
    fn set_embeddings(&mut self, embeddings: Embeddings);
    /// An independent copy of this catalog.
    fn duplicate(&self) -> Box<dyn Catalog>;
}

/// The `<authorization identifier>` that owns the schema `schema`
/// [ISO/IEC 9075:1992 §11.1 GR2], as `catalog` keeps it; for one it does
/// not keep, a system table's or none, an identifier of its own name, as
/// every schema's owner was in SQL-86 [X3.135-1986 §4.15].
pub fn owner_of<'a>(catalog: &'a dyn Catalog, schema: &'a str) -> &'a str {
    if let Some(owner) = catalog.schema_owner(schema) {
        return owner;
    }
    // [ISO/IEC 9075-4:2006 draft §12.8 GR3b] A module's declared tables'
    // owner is its schema's.
    module_of_schema(schema).and_then(|module| catalog.schema_owner(&module.auth)).unwrap_or(schema)
}

/// [ISO/IEC 9075-2:2023 §4.42.4] The cell of `auth` alone, against the
/// roles `catalog` keeps: whose privileges are those applicable for `auth`,
/// as a definition's owner's and a grantor's are.
pub fn sole<'a>(catalog: &'a dyn Catalog, auth: &'a str) -> Who<'a> {
    Who::sole(catalog.roles(), auth)
}
