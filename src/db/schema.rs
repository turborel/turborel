//! Schema definition: `<schema>` and the tables, columns, UNIQUE
//! constraints and privileges it defines [X3.135-1986 §6.1-§6.4, §6.6], and
//! the \[EXTENSION\] `create_index`. Split from db.rs (RULES.md R-SIZE-2).

use std::mem;
use std::sync::Arc;

use super::alter::Made;
use super::constraints::{self, Pending};
use super::defaults::default_of;
use super::indexes::IndexSpec;
use super::{Completion, Database, State, api};
use crate::ast::{Action, PrivilegeDefinition, Schema, TableDefinition, TableElement};
use crate::binder::{no_column, qualify};
use crate::catalog::{Actions, Catalog, ColumnDesc, IndexDesc, PrivilegeDesc, QualifiedName, TableDesc, TemporaryKind, applicable, merged};
use crate::engine::rows::ROW_STORE;
use crate::error::{At, Code, Result, SqlError, SqlState, fail};
use crate::index::Index;
use crate::log::record::Change;
use crate::parser::Parser;
use crate::settings::CATALOG_NAME;
use crate::storage::chosen;
use crate::txn::Timestamp;
use crate::value::Value;

/// [X3.135-1986 §6.6 SR1, SR2] The privileges the `<action>`s `list`
/// specify on `table`: UPDATE on each column of a `<grant column list>`,
/// which are columns of the table, or on every column without one.
pub(super) fn granted_actions(list: &[Action], table: &TableDesc) -> Result<Actions> {
    let mut actions = Actions::default();
    for a in list {
        match a {
            Action::Select => actions.select = true,
            Action::Insert(None) => actions.insert.extend(0..table.columns.len()),
            Action::Insert(Some(names)) => actions.insert.extend(columns_of(table, names)?),
            Action::Delete => actions.delete = true,
            Action::Update(None) => actions.update.extend(0..table.columns.len()),
            Action::Update(Some(names)) => actions.update.extend(columns_of(table, names)?),
            Action::References(None) => actions.references.extend(0..table.columns.len()),
            Action::References(Some(names)) => actions.references.extend(columns_of(table, names)?),
            // [ISO/IEC 9075:1992 §11.36 SR1] USAGE is a domain's.
            Action::Usage => {
                return rule(format!("[ISO/IEC 9075:1992 §11.36 SR1] USAGE is granted on a domain, not on table {}", table.name));
            }
        }
    }
    Ok(actions)
}

/// The positions of the columns `names` of `table` [X3.135-1986 §6.6 SR2,
/// ISO/IEC 9075:1992 §10.3 SR3].
fn columns_of(table: &TableDesc, names: &[String]) -> Result<Vec<usize>> {
    names
        .iter()
        .map(|c| {
            table.column(c).ok_or_else(|| no_column(&table.name, format!("[X3.135-1986 §6.6 SR2] {} has no column {c}", table.name), c))
        })
        .collect()
}

/// A Syntax Rule violation.
fn rule<T>(message: String) -> Result<T> {
    Err(SqlError::new(Code::SyntaxRule, message))
}

impl Database {
    /// [X3.135-1986 §6.1; ISO/IEC 9075:1992 §11.1] `<schema definition>`. A
    /// schema is a persistent object created by implementor-defined means
    /// [X3.135-1986 §3.3, §4.6]; it is not part of any transaction. Run by
    /// the operator, outside any session, it names its AUTHORIZATION, its
    /// owner; `Session::direct` runs one owned by the session's
    /// authorization identifier.
    ///
    /// # Errors
    ///
    /// Any error of the `<schema>`; it then has no effect [X3.135-1986 §3.3].
    /// SQLCODE -103 if it names no AUTHORIZATION [ISO/IEC 9075:1992 §11.1
    /// SR2b], or a catalog other than the one there is \[SR3\].
    /// A privilege it would grant that its owner cannot is not granted, and
    /// it completes with a warning [ISO/IEC 9075:1992 §11.36 GR14, GR15].
    pub fn schema(&self, sql: &str) -> Result<Completion> {
        self.schema_of(sql, None)
    }

    /// `schema`, the `<schema definition>` `sql` owned by `implicit` if it
    /// names no AUTHORIZATION [ISO/IEC 9075:1992 §11.1 SR2b].
    pub(super) fn schema_of(&self, sql: &str, implicit: Option<&str>) -> Result<Completion> {
        // Published before a checkpoint can see its record or its tables.
        let current = self.snapshot()?;
        let _gate = current.tx.gate.enter();
        self.noted(self.change(|st| {
            let (owner, engines, warned, asserted) = st.schema(sql, implicit)?;
            st.check_new_assertions(&asserted)?;
            // Logged before it is published (RULES.md D25).
            st.log_definition(None, Change::Schema { sql: sql.into(), owner, engines })?;
            Ok(if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED) } else { Completion::Success })
        }))
    }
}

/// What a `<schema definition>` made: its owner; each table, with the
/// engine it was placed in; whether a privilege it would grant was not;
/// and its assertions, to check (`db/assertions.rs`).
pub(super) type SchemaMade = (String, Vec<(String, String)>, bool, Vec<QualifiedName>);

impl State {
    /// \[EXTENSION\] A secondary index on `columns` of table `qn`, built over
    /// every version of its rows: for recovery, and for an engine leaving
    /// concurrency to the coordinator, whose caller holds the table's
    /// exclusive lock (`Database::create_index`).
    pub(super) fn create_index(&mut self, spec: &IndexSpec) -> Result<()> {
        let (index, cols, built) = self.define_index(spec)?;
        if !built {
            self.load_index(&*index, &spec.table, &cols, Timestamp::MAX)?;
        }
        Ok(())
    }

    /// \[EXTENSION\] Defines a secondary index on `columns` of table `qn`:
    /// in the catalog, with its structure, empty unless reopened, so every
    /// write from this snapshot on keeps it up to date. Returns the
    /// structure, its columns' positions, and whether it was reopened.
    pub(super) fn define_index(&mut self, spec: &IndexSpec) -> Result<(Arc<dyn Index>, Vec<usize>, bool)> {
        let IndexSpec { name, table: qn, columns, method, options } = spec;
        let desc = self.catalog.table(qn).cloned().ok_or_else(|| api(format!("[EXTENSION] no table {qn}")))?;
        let cols = columns
            .iter()
            .map(|c| desc.column(c).ok_or_else(|| api(format!("[EXTENSION] {qn} has no column {c}"))))
            .collect::<Result<Vec<_>>>()?;
        if self.catalog.index(name).is_some() {
            return fail(Code::Api, format!("[EXTENSION] an index named {name} already exists"));
        }
        let m = Arc::clone(self.methods.get(method).ok_or_else(|| api(format!("[EXTENSION] no index method {method}")))?);
        let declared = self.index_options_of(method);
        let mut taken: Vec<(String, Value)> = Vec::new();
        for (option, value) in options {
            // As for a table's (`State::stored`): kept as written while the
            // database is opened, if no plug-in declares it yet.
            if self.reopening.is_some() && !declared.iter().any(|o| o.name == *option) {
                taken.push((option.clone(), value.clone()));
                taken.sort_by(|a, b| a.0.cmp(&b.0));
                continue;
            }
            taken = chosen(method, (&declared, &taken), (option, Some(value)))?;
        }
        let ix = IndexDesc {
            name: name.clone(),
            table: qn.clone(),
            columns: cols.clone(),
            method: method.clone(),
            unique: false,
            options: taken,
        };
        let (index, built) = self.index_structure(&*m, (&ix, &desc), &desc.types(&cols).unwrap_or_default())?;
        self.indexes.insert(name.clone(), Arc::clone(&index));
        let mut catalog = self.catalog.duplicate();
        catalog.add_index(ix);
        self.catalog = Arc::from(catalog);
        // Writers whose snapshot predates the index must not write the table
        // without maintaining it (`State::prepare_write`).
        self.lock_of(qn)?.definitions_changed(self.generation);
        Ok((index, cols, built))
    }

    /// The one catalog's name, the setting CATALOG_NAME [ISO/IEC 9075:1992
    /// §5.4 SR20, §21.2.2].
    pub(crate) fn catalog_name(&self) -> String {
        self.settings.get(CATALOG_NAME).to_string().trim_end().to_owned()
    }

    /// [X3.135-1986 §6.1] A `<schema>`. Every rule is checked before anything
    /// changes, so a failing `<schema>` has no effect [X3.135-1986 §3.3].
    /// Its owner is its AUTHORIZATION, or, naming none, `implicit`, the
    /// session's authorization identifier [ISO/IEC 9075:1992 §11.1 SR2b].
    /// Returns its owner, and each table it created, with the engine it was
    /// placed in, for the log (RULES.md D25); and whether a privilege it
    /// would grant was not [ISO/IEC 9075:1992 §11.36 GR14, GR15].
    pub(super) fn schema(&mut self, sql: &str, implicit: Option<&str>) -> Result<SchemaMade> {
        let ours = self.catalog_name();
        let s = Parser::new(sql)?.in_catalog(&ours).schema()?;
        if let Some(other) = s.catalog.as_ref().filter(|c| **c != ours) {
            let msg = format!("[ISO/IEC 9075:1992 §11.1 SR3] the one catalog is {ours}, not {other}");
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_CATALOG_NAME));
        }
        let Some(owner) = s.auth.as_deref().or(implicit).map(str::to_owned) else {
            return rule(
                "[ISO/IEC 9075:1992 §11.1 SR2b] a <schema definition> run outside a session names its AUTHORIZATION: there is no session's to imply"
                    .to_owned(),
            );
        };
        // [ISO/IEC 9075:1992 §5.4] MODULE qualifies a declared local
        // temporary table (`db/declared.rs`).
        if s.name == crate::lexer::kw::MODULE {
            return rule(
                "[DESIGN] MODULE names a session's declared local temporary tables, not a schema [ISO/IEC 9075:1992 §5.4]".to_owned(),
            );
        }
        // [ISO/IEC 9075:1992 §21.2.1, §21.3.2] The schemas of the system
        // tables are the database's.
        if self.catalog.schema_exists(&s.name) || self.system.keys().any(|t| t.auth == s.name) {
            return rule(format!(
                "[X3.135-1986 §6.1 SR1; ISO/IEC 9075:1992 §11.1 SR3, §21.2.1, §21.3.2] a schema {} already exists",
                s.name
            ));
        }
        self.schema_domains(&s, &owner)?;
        let (mut descs, mut pending): (Vec<TableDesc>, Vec<Vec<Pending>>) = (Vec::new(), Vec::new());
        for table in &s.tables {
            let (desc, keys) = self.describe_table((&s.name, &s.name), table, &descs, &pending).or_at(table.name.at)?;
            descs.push(desc);
            pending.push(keys);
        }
        // Views, foreign keys, CHECKs and grants are checked in a catalog
        // with the schema's tables, and its views as each is defined, all of
        // them created at once [ISO/IEC 9075:1992 §11.1 GR3]: a CHECK may
        // read a view; the catalog is put back after.
        let mut staged = self.catalog.duplicate();
        if staged.schema_exists(&s.name) {
            descs.iter().cloned().for_each(|d| staged.add_table(d));
        } else {
            staged.create_schema(&s.name, &owner, descs.clone());
        }
        let saved = mem::replace(&mut self.catalog, Arc::from(staged));
        let defined = self.define_views(&s.name, &s.views).and_then(|()| {
            self.bind_domains(&s.name)?;
            self.complete_tables(&s.name, &mut descs, &pending)?;
            self.schema_grants(&s, &owner)
        });
        let with_views = mem::replace(&mut self.catalog, saved);
        let (granted, warned): (Vec<_>, Vec<_>) = defined?.into_iter().unzip();
        self.check_new_objects(&descs)?;
        self.apply_schema(&*with_views, &descs, granted.concat())?;
        let asserted = self.schema_assertions(&s)?;
        Ok((owner, descs.into_iter().map(|d| (d.name.ident, d.engine)).collect(), warned.contains(&true), asserted))
    }

    /// [ISO/IEC 9075:1992 §11.21] The domains of `<schema>` `s`, owned by
    /// `owner`, made first, which its tables' columns may be of: a failing
    /// `<schema>` is discarded whole (`Database::change`).
    fn schema_domains(&mut self, s: &Schema, owner: &str) -> Result<()> {
        if s.domains.is_empty() {
            return Ok(());
        }
        let mut catalog = self.catalog.duplicate();
        catalog.create_schema(&s.name, owner, Vec::new());
        self.catalog = Arc::from(catalog);
        for d in &s.domains {
            self.make_domain(&s.name, d, false)?;
        }
        Ok(())
    }

    /// The grants of `<schema>` `s`, by its owner `owner`: each table's
    /// privileges granted, and whether any was not; USAGE on a domain is
    /// granted in its descriptor [§11.36].
    fn schema_grants(&mut self, s: &Schema, owner: &str) -> Result<Vec<(Vec<PrivilegeDesc>, bool)>> {
        let mut granted = Vec::new();
        for p in &s.privileges {
            if p.domain {
                let done = self.grant_usage(owner, &s.name, p).or_at(p.table.at)?;
                granted.push((Vec::new(), done.completion != Completion::Success));
            } else {
                granted.push(self.describe_grant((owner, &s.name), p).or_at(p.table.at)?);
            }
        }
        Ok(granted)
    }

    /// [X3.135-1986 §6.2, §6.3; ISO/IEC 9075:1992 §11.3, §11.4] The
    /// description of one `<table definition>` of the schema `auth`, its
    /// names of schema `names` [§5.4 SR8], after the tables `earlier` in the
    /// same schema, with the foreign keys `keys` of each: with its
    /// constraints, and its foreign keys to resolve once the schema's tables
    /// are known.
    pub(super) fn describe_table(
        &self,
        (auth, names): (&str, &str),
        t: &TableDefinition,
        earlier: &[TableDesc],
        keys: &[Vec<Pending>],
    ) -> Result<(TableDesc, Vec<Pending>)> {
        if let Some(a) = t.name.auth.as_ref().filter(|a| *a != auth) {
            return rule(format!("[X3.135-1986 §6.2 SR1] table {a}.{} is not in schema {auth}", t.name.ident));
        }
        if earlier.iter().any(|d| d.name.ident == t.name.ident) {
            return rule(format!("[X3.135-1986 §6.2 SR2] table {} is defined twice", t.name.ident));
        }
        let mut columns: Vec<ColumnDesc> = Vec::new();
        for c in t.elements.iter().filter_map(|el| if let TableElement::Column(c) = el { Some(c) } else { None }) {
            if columns.iter().any(|x| x.name == c.name) {
                return rule(format!("[X3.135-1986 §6.3 SR1] column {} is defined twice in {}", c.name, t.name.ident)).or_at(c.at);
            }
            // [ISO/IEC 9075:1992 §11.4 SR5, AR1] its data type, or its domain's.
            let (data_type, domain) = self.column_type(names, &c.data_type).or_at(c.at)?;
            let default = c.default.as_ref().map(|o| default_of(data_type, o)).transpose().or_at(c.at)?;
            columns.push(ColumnDesc { name: c.name.clone(), data_type, not_null: false, default, domain });
        }
        if columns.is_empty() {
            return rule(format!("[ISO/IEC 9075:1992 §11.3 SR5] table {} has no column", t.name.ident));
        }
        let name = QualifiedName { auth: auth.to_owned(), ident: t.name.ident.clone() };
        let taken = earlier
            .iter()
            .flat_map(|d| d.constraints.iter().map(|c| c.name.clone()))
            .chain(keys.iter().flatten().map(|p| p.name.clone()))
            .collect();
        let (mut constraints, pending) = constraints::describe(&name, &columns, &t.elements, taken)?;
        // [§11.21 GR5, GR6] its domains' constraints, on their columns.
        constraints.extend(self.domain_checks(&name, &columns));
        // [ISO/IEC 9075:1992 §4.9] A temporary table's rows are each
        // session's, in the row store's memory (`db/temporary.rs`): its
        // definition names no engine or options.
        if t.temporary.is_some() && (t.storage.engine.is_some() || !t.storage.options.is_empty()) {
            return rule(format!("[DESIGN] {name} is temporary: its rows are each session's, in memory, so it names no engine or options"));
        }
        let placed = || self.placement.get(&name).map_or_else(|| self.table_engine().to_owned(), Clone::clone);
        let engine = if t.temporary.is_some() { ROW_STORE.to_owned() } else { placed() };
        let desc = TableDesc { name, columns, constraints, engine, options: Vec::new(), generation: 0, temporary: t.temporary };
        // [EXTENSION] its engine and options, if written (RULES.md D41).
        let mut desc = self.stored(&desc, &t.storage)?;
        desc.refresh_not_null();
        Ok((desc, pending))
    }

    /// The tables `descs`, their names of schema `auth`, in the catalog now, completed:
    /// each one's foreign keys `pending` resolved [ISO/IEC 9075:1992
    /// §11.8], and its CHECK conditions bound [§11.9].
    pub(super) fn complete_tables(&self, auth: &str, descs: &mut [TableDesc], pending: &[Vec<Pending>]) -> Result<()> {
        for (desc, keys) in descs.iter_mut().zip(pending) {
            for p in keys {
                let resolved = self.resolve_foreign_key(auth, desc, p)?;
                desc.constraints.push(resolved);
            }
            self.bind_checks(desc)?;
        }
        Ok(())
    }

    /// [X3.135-1986 §6.6; ISO/IEC 9075:1992 §11.36] What a `<grant
    /// statement>` `p` of `grantor`, its names in schema `schema`, grants
    /// each grantee on a table or view of the catalog: of the privileges
    /// it names, or all for ALL PRIVILEGES, those `grantor` holds with grant
    /// option [SR3, GR2, GR3]; and whether one it names, or for ALL
    /// PRIVILEGES any, was not granted [GR14, GR15]. SQL-86 refused such a
    /// grant [X3.135-1986 §6.6 SR5]; SQL-92 grants the rest, with a warning.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such table or view [§11.36 SR2], or a
    /// column named is not one of it [§10.3 SR3]; -104 if `grantor` holds
    /// no privilege on it [§11.36 AR1].
    pub(super) fn describe_grant(&self, (grantor, schema): (&str, &str), p: &PrivilegeDefinition) -> Result<(Vec<PrivilegeDesc>, bool)> {
        let name = qualify(&p.table, schema);
        let table = self.object(&name)?;
        // [§10.3 SR2] ALL PRIVILEGES, of a temporary table.
        if table.temporary.is_some() && p.actions.is_some() {
            return rule(format!("[ISO/IEC 9075:1992 §10.3 SR2] {name} is temporary: its privileges are granted as ALL PRIVILEGES"));
        }
        let (held, grantable) = applicable(&*self.catalog, grantor, &table.name);
        if held.is_empty() {
            return fail(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.36 AR1] {grantor} holds no privilege on {name}"));
        }
        let asked = match &p.actions {
            None => grantable.clone(),
            Some(list) => granted_actions(list, &table)?,
        };
        let actions = asked.common(&grantable);
        let warned = if p.actions.is_some() { actions != asked } else { actions.is_empty() };
        if actions.is_empty() {
            return Ok((Vec::new(), warned));
        }
        let granted = p.grantees.iter().map(|grantee| PrivilegeDesc {
            grantor: grantor.to_owned(),
            grantee: grantee.clone(),
            table: name.clone(),
            actions: actions.clone(),
            grantable: p.grant_option,
        });
        Ok((granted.collect(), warned))
    }

    /// The table or view `name`, as a table's description: a view's has its
    /// columns and nothing else.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is neither [X3.135-1986 §5.4 SR5].
    pub(super) fn object(&self, name: &QualifiedName) -> Result<TableDesc> {
        // [ISO/IEC 9075:1992 §10.3 SR1] not a declared local temporary table.
        if self.catalog.table(name).is_some_and(|t| t.temporary.is_some_and(|t| t.kind == TemporaryKind::Declared)) {
            return rule(format!(
                "[ISO/IEC 9075:1992 §10.3 SR1] {name} is a declared local temporary table: no privilege on it is granted or revoked"
            ));
        }
        match (self.catalog.table(name), self.catalog.view(name)) {
            (Some(t), _) => Ok(t.clone()),
            (None, Some(v)) => Ok(TableDesc {
                name: name.clone(),
                columns: v.columns.clone(),
                constraints: Vec::new(),
                engine: String::new(),
                options: Vec::new(),
                generation: 0,
                temporary: None,
            }),
            (None, None) => rule(format!("[X3.135-1986 §5.4 SR5] table {name} is not defined")),
        }
    }

    /// \[DESIGN\] What applying `descs` needs and could not do part way: each
    /// engine and the constraint index method registered, and every constraint
    /// index name free (the names are unambiguous, but `create_index` may have
    /// used one).
    pub(super) fn check_new_objects(&self, descs: &[TableDesc]) -> Result<()> {
        if !self.methods.contains_key(self.constraint_method()) {
            return fail(Code::Api, format!("[DESIGN] no index method {}", self.constraint_method()));
        }
        for d in descs {
            if !self.engines.contains_key(&d.engine) {
                return fail(Code::Api, format!("[DESIGN] no storage engine {} for {}", d.engine, d.name));
            }
            for k in 1..=d.unique().count() {
                let name = constraint_index_name(&d.name, k);
                if self.catalog.index(&name).is_some() {
                    return fail(
                        Code::Api,
                        format!("[DESIGN] the index name {name}, needed for a UNIQUE constraint of {}, is in use", d.name),
                    );
                }
            }
        }
        Ok(())
    }

    /// Records the checked tables, their storage, and an index for each UNIQUE
    /// constraint [X3.135-1986 §6.1, §6.4 GR2]. `check_new_objects` has found everything it uses.
    fn apply_schema(&mut self, staged: &dyn Catalog, descs: &[TableDesc], privileges: Vec<PrivilegeDesc>) -> Result<()> {
        let mut catalog = staged.duplicate();
        let tables: Vec<QualifiedName> = privileges.iter().map(|p| p.table.clone()).collect();
        catalog.grant(privileges);
        for table in tables {
            let all = catalog.privileges(&table).into_iter().cloned().collect();
            catalog.set_privileges(&table, merged(all));
        }
        for d in descs {
            // With its foreign keys, which the staged catalog lacks.
            catalog.replace_table(d.clone());
            self.place_table(&mut *catalog, d, None)?;
        }
        self.catalog = Arc::from(catalog);
        Ok(())
    }

    /// Gives table `d`, checked, its storage, its lock, and an index for
    /// each UNIQUE constraint, recorded in `catalog`: `made`'s, if given,
    /// else new. What it was given.
    pub(super) fn place_table(&mut self, catalog: &mut dyn Catalog, d: &TableDesc, made: Option<&Made>) -> Result<Made> {
        let storage = if let Some(storage) = made.and_then(|m| m.storage.clone()) {
            storage
        } else {
            let engine = Arc::clone(self.engines.get(&d.engine).ok_or_else(|| api(format!("[DESIGN] no storage engine {}", d.engine)))?);
            self.table_storage(&*engine, d)?
        };
        self.storage.insert(d.name.clone(), Arc::clone(&storage));
        self.locks.insert(d.name.clone(), self.tx.name_lock(&d.name));
        let method = Arc::clone(
            self.methods
                .get(self.constraint_method())
                .ok_or_else(|| api(format!("[DESIGN] no index method {}", self.constraint_method())))?,
        );
        let mut indexes = Vec::new();
        for (k, cols) in (1..).zip(d.unique()) {
            let name = constraint_index_name(&d.name, k);
            let types = d.types(cols).ok_or_else(|| api(format!("[DESIGN] a UNIQUE constraint of {} is on a column it lacks", d.name)))?;
            let index = IndexDesc {
                name: name.clone(),
                table: d.name.clone(),
                columns: cols.to_vec(),
                method: self.constraint_method().to_owned(),
                unique: true,
                options: Vec::new(),
            };
            let structure = match made.and_then(|m| m.indexes.iter().find(|(n, _)| *n == name)) {
                Some((_, structure)) => Arc::clone(structure),
                // A new table's index starts empty; a reopened one's is kept, or built.
                None => self.index_structure(&*method, (&index, d), &types)?.0,
            };
            self.indexes.insert(name.clone(), Arc::clone(&structure));
            indexes.push((name, structure));
            catalog.add_index(index);
        }
        Ok(Made { storage: Some(storage), indexes })
    }
}

/// \[DESIGN\] The name of the index that enforces the `k`-th UNIQUE constraint
/// of `table`, counting from 1. A `<delimited identifier>` may hold a `.`
/// [ISO/IEC 9075:1992 §5.2], so two tables could get the same name: a name
/// in use is refused (`check_new_objects`), never shared.
pub(super) fn constraint_index_name(table: &QualifiedName, k: usize) -> String {
    format!("{}.{}.UNIQUE{k}", table.auth, table.ident)
}

// Unit tests: what `check_new_objects` and `apply_schema` refuse, and that a
// refused `<schema>` changes nothing (RULES.md R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::{ConstraintDesc, ConstraintKind, MemoryCatalog};
    use crate::engine::rows::ROW_STORE;
    use crate::index::BTREE_METHOD;
    use crate::value::DataType;

    fn state() -> State {
        State::with_builtins(Box::new(MemoryCatalog::default()))
    }

    fn table(engine: &str, unique: bool) -> TableDesc {
        let column = ColumnDesc {
            name: "K".into(),
            data_type: DataType::Exact { precision: 10, scale: 0 },
            not_null: true,
            default: None,
            domain: None,
        };
        let name = QualifiedName { auth: "A".into(), ident: "T".into() };
        let key = ConstraintDesc {
            name: QualifiedName { auth: "A".into(), ident: "T_UNIQUE1".into() },
            kind: ConstraintKind::Unique { columns: vec![0], primary: false },
            attributes: crate::catalog::ConstraintAttributes::default(),
            domain: None,
        };
        TableDesc {
            name,
            columns: vec![column],
            constraints: if unique { vec![key] } else { vec![] },
            engine: engine.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        }
    }

    /// Verifies: [DESIGN] a `<schema>` needing an engine or constraint index
    /// method that is not registered is refused with SQLCODE -910 before
    /// anything changes [X3.135-1986 §3.3].
    #[test]
    fn unknown_engines_and_methods_are_refused_before_any_change() {
        let mut st = state();
        assert_eq!(st.check_new_objects(&[table("nope", false)]).err().map(|e| e.code), Some(Code::Api));
        st.settings.set("UNIQUE_INDEX_METHOD", Some(&Value::Char("nope".into()))).unwrap();
        assert_eq!(st.check_new_objects(&[table(ROW_STORE, true)]).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(
            st.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INT NOT NULL UNIQUE)", None).err().map(|e| e.code),
            Some(Code::Api)
        );
        assert!(!st.catalog.schema_exists("A"), "the refused <schema> left no trace");
    }

    /// Verifies: [DESIGN] `apply_schema` reports an unknown engine or method
    /// rather than panicking, if called without `check_new_objects`.
    #[test]
    fn apply_schema_reports_what_it_cannot_find() {
        let mut st = state();
        let catalog = Arc::clone(&st.catalog);
        assert_eq!(st.apply_schema(&*catalog, &[table("nope", false)], vec![]).err().map(|e| e.code), Some(Code::Api));
        let mut no_method = state();
        no_method.settings.set("UNIQUE_INDEX_METHOD", Some(&Value::Char("nope".into()))).unwrap();
        let other = Arc::clone(&no_method.catalog);
        assert_eq!(no_method.apply_schema(&*other, &[table(ROW_STORE, true)], vec![]).err().map(|e| e.code), Some(Code::Api));
    }

    /// Verifies: [DESIGN] `create_index` on a catalog table without storage
    /// reports SQLCODE -910 instead of panicking.
    #[test]
    fn create_index_without_storage_is_reported() {
        let mut catalog = MemoryCatalog::default();
        catalog.create_schema("A", "A", vec![table(ROW_STORE, false)]);
        let mut st = State::with_builtins(Box::new(catalog));
        let name = QualifiedName { auth: "A".into(), ident: "T".into() };
        let spec = IndexSpec { name: "I".into(), table: name, columns: vec!["K".into()], method: BTREE_METHOD.into(), options: Vec::new() };
        assert_eq!(st.create_index(&spec).err().map(|e| e.code), Some(Code::Api));
    }
}
