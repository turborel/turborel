//! \[EXTENSION\] A table's engine and options (RULES.md D41): those `CREATE
//! TABLE ... USING engine WITH (...)` gives it, and those `ALTER TABLE ...
//! SET (...)` and `ALTER TABLE ... USING engine WITH (...)` change. A
//! change of options that change in place is published with the new
//! descriptor, the table's storage told (`TableStorage::reconfigure`); a
//! change of engine, or of an option that needs a rebuild, gives the table
//! a new generation of its storage, made with them, and its rows are
//! copied there, as for a change of its columns (`db/alter.rs`). Split from
//! db/alter.rs (RULES.md R-SIZE-2).

use super::{Made, Reshape};
use crate::ast::{AlterAction, Literal, OptionValue, TableStorage};
use crate::catalog::{QualifiedName, TableDesc};
use crate::db::ddl::Dropped;
use crate::db::{State, rule};
use crate::error::{At, Code, Result, SqlError};
use crate::settings::{Kind, column_names};
use crate::storage::{TableOption, chosen, needs_rebuild};
use crate::txn::Txn;
use crate::value::Value;

/// A Syntax Rule of this extension broken (SQLCODE -103).
fn refused(message: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {message}"))
}

/// The names an option of columns (`Kind::Columns`) gives, `value`.
pub(in crate::db) fn columns_named(value: &Value) -> Vec<String> {
    let Value::Char(names) = value else { return Vec::new() };
    column_names(names).unwrap_or_default()
}

/// Checks that each option of `declared` that names columns of the table
/// `next` names its columns, each once.
///
/// # Errors
///
/// SQLCODE -103 for a name of no column of the table, or one named twice.
fn named_columns(declared: &[TableOption], next: &TableDesc) -> Result<()> {
    for (name, value) in &next.options {
        if !declared.iter().any(|o| o.name == *name && matches!(o.kind, Kind::Columns { .. })) {
            continue;
        }
        let names = columns_named(value);
        for (n, column) in names.iter().enumerate() {
            if next.column(column).is_none() {
                return Err(refused(&format!("{} has no column {column}, for the table option {name}", next.name)));
            }
            if names.iter().take(n).any(|c| c == column) {
                return Err(refused(&format!("the table option {name} names the column {column} twice")));
            }
        }
    }
    Ok(())
}

/// The value a `<table option>` is written with.
pub(in crate::db) fn value_of(value: &OptionValue) -> Value {
    match value {
        OptionValue::Literal(Literal::Character(s)) | OptionValue::Word(s) => Value::Char(s.clone()),
        OptionValue::Literal(Literal::Exact(d)) => Value::Exact(*d),
        OptionValue::Literal(Literal::Approximate(x)) => Value::Approx(*x),
        OptionValue::Literal(Literal::Bit(b)) => Value::Bits(b.clone()),
        // An option is the database's, of no session: its datetime of UTC.
        OptionValue::Literal(Literal::Datetime(w)) => crate::binder::datetime_literal(w, 0),
        OptionValue::Literal(Literal::Interval(i)) => Value::Interval(*i),
    }
}

impl State {
    /// The registered engine named `name`, in any case.
    fn engine_named(&self, name: &str) -> Result<String> {
        let found = self.engines.keys().find(|k| k.eq_ignore_ascii_case(name));
        found.cloned().ok_or_else(|| refused(&format!("there is no storage engine {name}")))
    }

    /// The options a table of `engine` takes: the engine's, then those
    /// plug-ins declared for every table (`Database::declare_table_option`).
    pub(crate) fn options_of(&self, engine: &str) -> Vec<TableOption> {
        let own = self.engines.get(engine).map(|e| e.table_options()).unwrap_or_default();
        own.into_iter().chain(self.table_options.iter().map(|(_, o)| o.clone())).collect()
    }

    /// `desc` with the engine and options `storage` writes: its engine, if
    /// it names one, else `desc`'s; the options it sets added to `desc`'s,
    /// of which those of its engine are kept only if the engine does not
    /// change. While the database is opened from its files, an option no
    /// plug-in declares yet is kept as written, so a table whose plug-in is
    /// registered later, or not again, is made all the same.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for an engine not registered, an option not declared
    /// or given twice, or a value not of its option's kind.
    pub(in crate::db) fn stored(&self, desc: &TableDesc, storage: &TableStorage) -> Result<TableDesc> {
        let engine = match &storage.engine {
            Some((name, at)) => self.engine_named(name).or_at(*at)?,
            None => desc.engine.clone(),
        };
        let declared = self.options_of(&engine);
        let engines = self.engines.get(&desc.engine).map(|e| e.table_options()).unwrap_or_default();
        let kept = |name: &str| engine == desc.engine || !engines.iter().any(|o| o.name == name);
        let mut options: Vec<(String, Value)> = desc.options.iter().filter(|(n, _)| kept(n)).cloned().collect();
        for (n, option) in storage.options.iter().enumerate() {
            if storage.options.iter().take(n).any(|o| o.name.eq_ignore_ascii_case(&option.name)) {
                return Err(refused(&format!("the table option {} is given twice", option.name))).or_at(option.at);
            }
            let value = option.value.as_ref().map(value_of);
            let name = option.name.to_ascii_uppercase();
            if self.reopening.is_some() && !declared.iter().any(|o| o.name == name) {
                options.retain(|(set, _)| *set != name);
                options.extend(value.map(|v| (name, v)));
                options.sort_by(|a, b| a.0.cmp(&b.0));
                continue;
            }
            options = chosen(&engine, (&declared, &options), (&option.name, value.as_ref())).or_at(option.at)?;
            self.registered_in(&declared, &mut options, &name).or_at(option.at)?;
        }
        let next = TableDesc { engine, options, ..desc.clone() };
        // A plug-in registered late, or not again, never stops the database
        // opening (RULES.md D41).
        if self.reopening.is_none() {
            named_columns(&declared, &next)?;
            self.check_encodings(&next)?;
        }
        Ok(next)
    }

    /// Makes the value of option `name` of `options`, if it is of
    /// encodings, name them as they are registered.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for an encoding not registered.
    fn registered_in(&self, declared: &[TableOption], options: &mut [(String, Value)], name: &str) -> Result<()> {
        if declared.iter().any(|o| o.name == name && matches!(o.kind, Kind::Encodings))
            && let Some((_, Value::Char(names))) = options.iter_mut().find(|(n, _)| n == name)
        {
            *names = self
                .encodings
                .canonical(names)?
                .map_err(|missing| refused(&format!("no encoding {missing} is registered, for the table option {name}")))?;
        }
        Ok(())
    }

    /// ALTER TABLE `qn` SET or USING, as `action` writes: in place, or by
    /// a rebuild, its storage `made`'s if given, else new. What it took
    /// away, what it made, and how the rows change, if they are copied.
    ///
    /// # Errors
    ///
    /// As `stored`; the engine's.
    pub(super) fn alter_storage(
        &mut self,
        qn: &QualifiedName,
        action: &AlterAction,
        (made, tx): (Option<&Made>, Option<&Txn>),
    ) -> Result<(Dropped, Made, Option<Reshape>)> {
        let AlterAction::Storage(storage) = action else { return Err(SqlError::new(Code::Api, "[DESIGN] not a storage action")) };
        let desc = self.base_table(qn)?;
        // As CREATE TABLE refuses them of one (`db/schema.rs`).
        if desc.temporary.is_some() {
            return Err(rule(format!(
                "[DESIGN] {qn} is temporary: its rows are each session's, in memory, so it names no engine or options"
            )));
        }
        let mut next = self.stored(&desc, storage)?;
        let declared = self.options_of(&next.engine);
        if next.engine == desc.engine && !needs_rebuild(&declared, &desc.options, &next.options) {
            let mut catalog = self.catalog.duplicate();
            catalog.replace_table(next.clone());
            self.catalog = std::sync::Arc::from(catalog);
            // Published, or replayed: the storage takes them now.
            if tx.is_none() {
                self.storage_of(qn)?.reconfigure(&next);
                for ix in self.catalog.indexes(qn) {
                    self.index_of(&ix.name)?.reconfigure((&next, ix));
                }
            }
            return Ok((Dropped::default(), Made::default(), None));
        }
        next.generation = desc.generation.saturating_add(1);
        let (dropped, built) = self.regenerate(qn, (&desc, next), (Reshape::Keep, Vec::new()), (made, action))?;
        Ok((dropped, built, Some(Reshape::Keep)))
    }
}
