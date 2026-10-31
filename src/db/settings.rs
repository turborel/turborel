//! \[DESIGN\] RULES.md D36: the settings (`settings.rs`) as the database and
//! its sessions set, list and read them. A database setting is a
//! definition: logged, outside any transaction, and replayed with the
//! others (`db/recovery.rs`); but a node's, never logged (D57). Split from db.rs (RULES.md R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use super::{Database, Session, State};
use crate::binder::Context;
use crate::diagnostics::{Event, Extension};
use crate::error::{Code, Result, SqlError};
use crate::log::record::Change;
use crate::params::Params;
use crate::settings::{self, Definition, Kind, LOCKS_WAIT, Listed, SESSION_COMMIT_AT_END};
use crate::storage::TableOption;
use crate::system::{self, SystemTable};
use crate::txn::{LockWait, SessionEnd};
use crate::value::Value;
use crate::value::datetime::Clocked;

impl State {
    /// Sets the database's value of the setting `name`, or its default
    /// again for none, and logs it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting, it is fixed once the
    /// database is created, the value is not of its kind, or it names an
    /// engine, index method, hash function or encodings not registered;
    /// the log's errors.
    pub(super) fn define_setting(&mut self, name: &str, value: Option<&Value>) -> Result<()> {
        let key = self.settings.known(name)?;
        let definition = self.settings.definition(key).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a setting lost its definition"))?;
        let (canonical, definition_is_node) = (definition.name.clone(), definition.node);
        if let Some(refused) = (definition.effect == settings::Effect::Creation).then(|| self.creation_refused(&canonical)).flatten() {
            return Err(SqlError::new(Code::Api, refused));
        }
        let mut value = value.cloned();
        if let Some(v) = &value {
            let given = settings::accepted(&canonical, &definition.kind, v)?;
            let missing = match (&definition.kind, &given) {
                (Kind::Engine, Value::Char(engine)) => !self.engines.contains_key(engine),
                (Kind::IndexMethod, Value::Char(method)) => !self.methods.contains_key(method),
                (Kind::HashFunction, Value::Char(hash)) => {
                    // Kept as it is registered.
                    let found = self.hashes.iter().find(|h| h.name().eq_ignore_ascii_case(hash));
                    value = found.map(|h| Value::Char(h.name().to_owned()));
                    found.is_none()
                }
                (Kind::Encodings, Value::Char(names)) => {
                    let found = self.encodings.canonical(names)?.ok();
                    value = found.clone().map(Value::Char);
                    found.is_none()
                }
                (
                    Kind::Engine
                    | Kind::IndexMethod
                    | Kind::HashFunction
                    | Kind::Encodings
                    | Kind::Integer { .. }
                    | Kind::Number { .. }
                    | Kind::Choice(_)
                    | Kind::Displacement
                    | Kind::Identifier
                    | Kind::Text { .. }
                    | Kind::Columns { .. },
                    _,
                ) => false,
            };
            if missing {
                return Err(SqlError::new(Code::Api, format!("[DESIGN] {canonical}: nothing is registered under the name {given}")));
            }
        }
        self.settings.set(&canonical, value.as_ref())?;
        // A node's value is the node's alone: never logged (RULES.md D57).
        if definition_is_node {
            return Ok(());
        }
        self.log_definition(None, Change::Setting { name: canonical, value })
    }

    /// Declares the setting of each optimizer rule that has none yet,
    /// ON by default, a session's to set for itself (`optimizer::switch_name`);
    /// one declared before, as of a rule added again, is taken again.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a setting of a rule's name is another kind's, or its
    /// name is too long for a setting's.
    pub(crate) fn switch_rules(&mut self) -> Result<()> {
        for rule in self.optimizer.unswitched() {
            let name = crate::optimizer::switch_name(rule);
            let key = match self.settings.key(&name) {
                Some(key) => key,
                None => self.settings.declare(Definition {
                    name: name.clone(),
                    kind: Kind::Choice(settings::SWITCHES),
                    default: Value::Char("ON".to_owned()),
                    per_session: true,
                    node: false,
                    effect: settings::Effect::Statement,
                    doc: format!("Whether the optimizer's rule {rule} rewrites plans: ON; or OFF, as to compare plans made without it."),
                })?,
            };
            if self.settings.definition(key).is_none_or(|d| d.kind != Kind::Choice(settings::SWITCHES)) {
                return Err(SqlError::new(Code::Api, format!("[DESIGN] {name} is a setting of another kind than a rule's")));
            }
            self.optimizer.switched(rule, key);
        }
        Ok(())
    }
}

impl Database {
    /// \[DESIGN\] Sets the database's value of the setting `name`, in any
    /// case, for statements that begin from now on, or transactions, as
    /// the setting says; kept with the definitions, so it holds when the
    /// database is opened again. A node's setting (`Definition::node`) is
    /// kept nowhere: its value lasts while the database runs (RULES.md
    /// D57).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting, it is fixed once the
    /// database is created, the value is not of its kind or range, or it
    /// names an engine, index method, hash function or encodings not
    /// registered.
    pub fn set_setting(&self, name: &str, value: &Value) -> Result<()> {
        self.change(|st| st.define_setting(name, Some(value)))?;
        self.emit(&Event::SettingChanged { name: name.to_ascii_uppercase(), value: Some(value.clone()) });
        self.apply_job_policy();
        self.apply_settings()
    }

    /// Gives the settings as they are now published to what reads them
    /// held: the memory budget its limit, MEMORY.BUDGET_BYTES's; the files
    /// their limit; the pages of a database in memory their pool's size,
    /// STORAGE.POOL_BYTES', and once their pool is made, the size it was
    /// made of as the one started with; and each plug-in the database's
    /// values (`configure_plugins`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub(super) fn apply_settings(&self) -> Result<()> {
        let st = self.snapshot()?;
        st.memory.set_limit(settings::budget_bytes(st.settings.get(settings::MEMORY_BUDGET_BYTES)));
        st.files.set_limit(usize::try_from(st.settings.integer(settings::FILES_MAX_OPEN)).unwrap_or(usize::MAX));
        // A database in memory makes its pool so, if it has not yet.
        st.pages.resize(settings::pool_bytes(st.settings.get(settings::POOL_BYTES)));
        if let Some(made) = st.pages.made_bytes() {
            st.settings.mark_started(
                settings::POOL_BYTES,
                Some(Value::Exact(crate::value::Decimal::new(i128::try_from(made).unwrap_or(i128::MAX), 0))),
            );
        }
        self.configure_plugins()
    }

    /// \[DESIGN\] Records the database's values of the settings `names`,
    /// taken as what reads them starts, as those it started with: a
    /// server's, as it starts listening. INFORMATION_SCHEMA.SETTINGS shows
    /// a value set apart from the one started with as waiting for a restart
    /// (RULES.md D36).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting, or the database is closed.
    pub fn settings_started(&self, names: &[&str]) -> Result<()> {
        let st = self.snapshot()?;
        for name in names {
            st.settings.mark_started(st.settings.known(name)?, None);
        }
        Ok(())
    }

    /// \[EXTENSION\] Notes the file at `path`, which the database does not
    /// open, but shows in INFORMATION_SCHEMA.FILES as KIND SERVER until the
    /// note is dropped: a server's own, as its diagnostics log (RULES.md
    /// D56). Whether it is open, and its reads and writes, are as the note
    /// is told; it is not under FILES.MAX_OPEN.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn note_file(&self, path: &Path, open: bool) -> Result<crate::files::Noted> {
        Ok(self.snapshot()?.files.note(path, open))
    }

    /// \[DESIGN\] The bytes its memory budget allows, and those its buffer
    /// pools hold of it (RULES.md D38).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn memory(&self) -> Result<(usize, usize)> {
        let st = self.snapshot()?;
        Ok((st.memory.limit(), st.memory.reserved()))
    }

    /// \[DESIGN\] Gives the setting `name` its default again, as
    /// `set_setting` sets it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting, or it is fixed once the
    /// database is created.
    pub fn reset_setting(&self, name: &str) -> Result<()> {
        self.change(|st| st.define_setting(name, None))?;
        self.emit(&Event::SettingChanged { name: name.to_ascii_uppercase(), value: None });
        self.apply_job_policy();
        self.apply_settings()
    }

    /// \[DESIGN\] The database's value of the setting `name`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting.
    pub fn setting(&self, name: &str) -> Result<Value> {
        let st = self.snapshot()?;
        let key = st.settings.known(name)?;
        Ok(st.settings.get(key).clone())
    }

    /// \[DESIGN\] Every setting, with the database's value of each.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn settings(&self) -> Result<Vec<Listed>> {
        Ok(self.snapshot()?.settings.list(&settings::Own::default()))
    }

    /// \[DESIGN\] Declares a setting of what serves the database, as a
    /// server's (RULES.md D42); a plug-in registered declares its own as it
    /// is registered (`Configurable`). A value the database was given for
    /// it before, as it was opened again, is its value.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a setting of its name is declared, its default is
    /// not of its kind, or it is a node's (`Definition::node`) and a
    /// session's or fixed at creation.
    pub fn declare_setting(&self, definition: Definition) -> Result<()> {
        let name = definition.name.clone();
        let done = self.change(|st| st.settings.declare(definition).map(|_| ()));
        self.registered((Extension::Setting, &name), done)
    }

    /// \[EXTENSION\] Declares `option`, an option of every table, for the
    /// plug-in `by`: an index method's, or any other's (RULES.md D41). Set
    /// and changed as an engine's options are, it takes effect for the
    /// statements that begin after; one that needs a rebuild makes the
    /// table and its indexes anew (`IndexMethod::create_on`), and one that
    /// does not tells them (`TableStorage::reconfigure`,
    /// `Index::reconfigure`). A table's value of it given before, as the
    /// database was opened again, is its value.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is not an `<identifier>`'s, or that of an
    /// option declared or of a registered engine's; if its default, or a
    /// table's value of it, is not of its kind.
    pub fn declare_table_option(&self, by: &str, option: TableOption) -> Result<()> {
        let name = option.name.clone();
        let done = self.change(|st| {
            let misuse = |what: String| SqlError::new(Code::Api, format!("[DESIGN] {what}"));
            if name.is_empty() || name.chars().count() > crate::lexer::MAX_IDENTIFIER_LENGTH {
                return Err(misuse(format!("{name} is not a table option's name")));
            }
            let engines = st.engines.values().flat_map(|e| e.table_options());
            if st.table_options.iter().any(|(_, o)| o.name == name) || engines.into_iter().any(|o| o.name == name) {
                return Err(misuse(format!("the table option {name} is declared")));
            }
            settings::accepted(&name, &option.kind, &option.default)?;
            // Values kept as written while the database was opened, taken
            // of the option's kind now.
            let mut catalog = st.catalog.duplicate();
            for t in st.catalog.tables() {
                let Some((_, v)) = t.options.iter().find(|(n, _)| *n == name) else { continue };
                let taken = settings::accepted(&name, &option.kind, v).map_err(|e| misuse(format!("{}: {}", t.name, e.msg)))?;
                let mut desc = t.clone();
                desc.options.iter_mut().filter(|(n, _)| *n == name).for_each(|(_, value)| *value = taken.clone());
                catalog.replace_table(desc);
            }
            st.catalog = Arc::from(catalog);
            st.table_options.push((by.to_owned(), option));
            Ok(())
        });
        self.registered((Extension::TableOption, &name), done)
    }

    /// \[EXTENSION\] Declares `option`, an option of every index, of any
    /// method, for the plug-in `by` (RULES.md D41), as
    /// `declare_table_option` does one of every table: set by `CREATE INDEX
    /// ... WITH (...)` and changed by ALTER INDEX, if it changes in place,
    /// the index told (`Index::reconfigure`); its method reads it in the
    /// index's descriptor (`IndexMethod::create_on`). An index's value of it
    /// given before, as the database was opened again, is its value.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is not an `<identifier>`'s, or that of an
    /// option declared or of a registered method's; if its default, or an
    /// index's value of it, is not of its kind.
    pub fn declare_index_option(&self, by: &str, option: TableOption) -> Result<()> {
        let name = option.name.clone();
        let done = self.change(|st| {
            let misuse = |what: String| SqlError::new(Code::Api, format!("[DESIGN] {what}"));
            if name.is_empty() || name.chars().count() > crate::lexer::MAX_IDENTIFIER_LENGTH {
                return Err(misuse(format!("{name} is not an index option's name")));
            }
            let methods = st.methods.values().flat_map(|m| m.index_options());
            if st.index_options.iter().any(|(_, o)| o.name == name) || methods.into_iter().any(|o| o.name == name) {
                return Err(misuse(format!("the index option {name} is declared")));
            }
            settings::accepted(&name, &option.kind, &option.default)?;
            // Values kept as written while the database was opened, taken
            // of the option's kind now.
            let mut catalog = st.catalog.duplicate();
            let indexes: Vec<_> = st.catalog.tables().iter().flat_map(|t| st.catalog.indexes(&t.name)).cloned().collect();
            for ix in indexes {
                let Some((_, v)) = ix.options.iter().find(|(n, _)| *n == name) else { continue };
                let taken = settings::accepted(&name, &option.kind, v).map_err(|e| misuse(format!("{}: {}", ix.name, e.msg)))?;
                let mut desc = ix.clone();
                desc.options.iter_mut().filter(|(n, _)| *n == name).for_each(|(_, value)| *value = taken.clone());
                catalog.remove_index(&desc.name);
                catalog.add_index(desc);
            }
            st.catalog = Arc::from(catalog);
            st.index_options.push((by.to_owned(), option));
            Ok(())
        });
        self.registered((Extension::IndexOption, &name), done)
    }

    /// \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] Adds or replaces a system
    /// table, under its schema and name (`system.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it would be of the Definition Schema, or named as
    /// a view of the Information Schema is; if a setting it declares is
    /// refused (`Configurable`, under its name, RULES.md D36); if called
    /// back from a plug-in, or a lock is poisoned.
    pub fn register_system_table(&self, table: Box<dyn SystemTable>) -> Result<()> {
        let table: Arc<dyn SystemTable> = Arc::from(table);
        let name = system::name_of(&*table);
        if name.auth == system::DEFINITION_SCHEMA || system::view(&name).is_some() {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {name} is the database's own")));
        }
        let shown = name.to_string();
        let done = self.change(|st| {
            st.declare_plugin(table.name(), table.configurable())?;
            st.system.insert(name, Arc::clone(&table));
            Ok(())
        });
        self.registered((Extension::SystemTable, &shown), done.and_then(|()| self.tell_plugin(table.configurable())))
    }
}

impl Session {
    /// \[DESIGN\] Sets this session's own value of the setting `name`, one
    /// a session may set for itself, over the database's, until it ends or
    /// resets it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting, it is the database's alone,
    /// or the value is not of its kind or range.
    pub fn set_setting(&self, name: &str, value: &Value) -> Result<()> {
        let st = self.db.snapshot()?;
        self.own.borrow_mut().set(&st.settings, name, Some(value))
    }

    /// \[DESIGN\] Takes the database's value of the setting `name` again.
    ///
    /// # Errors
    ///
    /// As `set_setting`.
    pub fn reset_setting(&self, name: &str) -> Result<()> {
        let st = self.db.snapshot()?;
        self.own.borrow_mut().set(&st.settings, name, None)
    }

    /// \[DESIGN\] The value of the setting `name` this session sees: its
    /// own, or the database's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting.
    pub fn setting(&self, name: &str) -> Result<Value> {
        let st = self.db.snapshot()?;
        let key = st.settings.known(name)?;
        Ok(st.settings.effective(key, &self.own.borrow()).0.clone())
    }

    /// \[DESIGN\] Every setting, with the value this session sees, and
    /// where it comes from.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn settings(&self) -> Result<Vec<Listed>> {
        Ok(self.db.snapshot()?.settings.list(&self.own.borrow()))
    }

    /// What `bind` gives, given the context of a statement of this session
    /// against `st` with `params`: its user and its own settings, which the
    /// system tables show.
    ///
    /// # Errors
    ///
    /// Those of `bind`.
    pub(super) fn bound<T>(&self, st: &State, params: &Params, bind: impl FnOnce(Context<'_>) -> Result<T>) -> Result<T> {
        let (own, schema) = (self.own.borrow(), self.schema.borrow());
        let now = Clocked::new(self.now.get(), params.noted());
        bind(Context { own: &own, schema: &schema, now, ..Context::new(st, &self.auth(), params) })
    }

    /// What ends its open transaction when it ends (`SESSION.COMMIT_AT_END`).
    pub(super) fn session_end(&self, st: &State) -> SessionEnd {
        settings::session_end(st.settings.effective(SESSION_COMMIT_AT_END, &self.own.borrow()).0)
    }

    /// `st`, its settings this session's own over the database's, so its
    /// statements read them where they read the database's (RULES.md D36):
    /// a copy, made only if it set one, never published.
    pub(super) fn with_own(&self, st: Arc<State>) -> Arc<State> {
        let own = self.own.borrow();
        if own.is_empty() {
            return st;
        }
        let mut copy = State::clone(&st);
        copy.settings = st.settings.overlaid(&own);
        Arc::new(copy)
    }

    /// Whether its transactions wait for locks (`LOCKS.WAIT`).
    pub(super) fn lock_wait(&self, st: &State) -> LockWait {
        settings::lock_wait(st.settings.effective(LOCKS_WAIT, &self.own.borrow()).0)
    }
}
