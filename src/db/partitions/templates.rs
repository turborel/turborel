//! \[EXTENSION\] CREATE INDEX, DROP INDEX and ALTER INDEX of a partitioned
//! table (RULES.md D158): the index is a template, kept with the table's
//! key, made as an index of its method, options and key on each partition
//! that holds rows, present and future, under a derived name. Each is
//! built while transactions go on writing its partition, as CREATE INDEX
//! builds one (`db/indexes.rs`), the partitioned table's shared lock held
//! so no partition is attached or made meanwhile; then the template is
//! recorded and logged. Replayed, the template makes its indexes again.
//! DROP INDEX and ALTER INDEX of the template are of each index it made.

use super::super::indexes::{IndexSpec, key_positions};
use super::super::{Database, State, api};
use crate::catalog::{DERIVED_SEPARATOR, IndexDesc, IndexTemplate, PartitionDesc, QualifiedName, derived_index_name};
use crate::error::Result;
use crate::log::record::Change;
use crate::storage::chosen;
use crate::txn::LockMode;
use crate::value::Value;

impl Database {
    /// The template `spec` describes, of the partitioned table it names,
    /// made: its index built on each partition, then it recorded and
    /// logged. If one fails, those built are dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is taken or holds a NUL; as `make_index`.
    pub(in crate::db) fn make_template(&self, spec: &IndexSpec) -> Result<()> {
        let current = self.snapshot()?;
        current.template_free(spec)?;
        let holder = current.begin();
        current.acquire(&holder, &spec.table, LockMode::AccessShare)?;
        let mut built: Vec<String> = Vec::new();
        let mut done = Ok(());
        for leaf in current.catalog.partitioning().leaves(&spec.table) {
            let one = IndexSpec { name: derived_index_name(&spec.name, &leaf), table: leaf, ..spec.clone() };
            done = self.build_index(&one, &|_, _| Ok(()));
            if done.is_err() {
                break;
            }
            built.push(one.name);
        }
        let done = done.and_then(|()| {
            self.change(|st| {
                st.add_template(spec)?;
                st.log_definition(Some(&holder), spec.change())
            })
        });
        if done.is_err() {
            for name in &built {
                let _ = self.change(|st| st.drop_index(name));
            }
        }
        done
    }
}

impl State {
    /// That the template `spec` may be made: its name no index's nor
    /// template's, holding no NUL.
    fn template_free(&self, spec: &IndexSpec) -> Result<()> {
        if spec.name.contains(DERIVED_SEPARATOR) || self.catalog.index(&spec.name).is_some() || self.template(&spec.name).is_some() {
            return Err(api(format!("[EXTENSION] an index named {} already exists, or the name holds a NUL", spec.name)));
        }
        Ok(())
    }

    /// The template `spec` describes recorded with its table's key; of a
    /// replayed CREATE INDEX, its index made on each partition too.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the table is not partitioned, a column is not
    /// there, or an option is not the method's.
    pub(in crate::db) fn add_template(&mut self, spec: &IndexSpec) -> Result<()> {
        let desc = self.catalog.table(&spec.table).cloned().ok_or_else(|| api(format!("[EXTENSION] no table {}", spec.table)))?;
        let columns = key_positions(&desc, &spec.columns, &spec.expressions)?;
        let declared = self.index_options_of(&spec.method);
        let mut options: Vec<(String, Value)> = Vec::new();
        for (option, value) in &spec.options {
            options = chosen(&spec.method, (&declared, &options), (option, Some(value)))?;
        }
        let template = IndexTemplate {
            name: spec.name.clone(),
            columns,
            method: spec.method.clone(),
            unique: false,
            options,
            expressions: spec.expressions.clone(),
            nulls: spec.nulls,
        };
        let Some(mut d) = self.catalog.partitioning().get(&spec.table).cloned() else {
            return Err(api(format!("[EXTENSION] {} is not partitioned", spec.table)));
        };
        let Some(key) = d.key.as_mut() else { return Err(api(format!("[EXTENSION] {} is not partitioned", spec.table))) };
        key.templates.push(template);
        let mut catalog = self.catalog.duplicate();
        catalog.partitioning_mut().put(d);
        self.catalog = std::sync::Arc::from(catalog);
        for leaf in self.catalog.partitioning().leaves(&spec.table) {
            let name = derived_index_name(&spec.name, &leaf);
            if self.catalog.index(&name).is_none() {
                self.create_index(&IndexSpec { name, table: leaf, ..spec.clone() })?;
            }
        }
        Ok(())
    }

    /// The template named `name`, of an index of a partitioned table, and
    /// that table.
    pub(crate) fn template(&self, name: &str) -> Option<(QualifiedName, IndexTemplate)> {
        self.catalog.partitioning().all().find_map(|d| {
            let t = d.key.as_ref()?.templates.iter().find(|t| t.name == name)?;
            Some((d.table.clone(), t.clone()))
        })
    }

    /// The indexes the template `name` of `table` made, each its name.
    pub(crate) fn made_of_template(&self, table: &QualifiedName, name: &str) -> Vec<String> {
        let leaves = self.catalog.partitioning().leaves(table);
        leaves.iter().map(|leaf| derived_index_name(name, leaf)).filter(|n| self.catalog.index(n).is_some()).collect()
    }

    /// The template `name` as an index of its partitioned table: what
    /// DROP INDEX and ALTER INDEX are given of it.
    pub(crate) fn template_index(&self, name: &str) -> Option<IndexDesc> {
        let (table, t) = self.template(name)?;
        Some(IndexDesc {
            name: t.name,
            table,
            columns: t.columns,
            method: t.method,
            unique: t.unique,
            options: t.options,
            collations: Vec::new(),
            nulls: t.nulls,
            expressions: t.expressions.iter().map(|(p, text)| crate::catalog::KeyExpression::unbound(*p, text.clone())).collect(),
        })
    }

    /// Drops the template `name`, with each index it made.
    ///
    /// # Errors
    ///
    /// As `drop_index`.
    pub(in crate::db) fn drop_template(&mut self, name: &str) -> Result<()> {
        let Some((table, _)) = self.template(name) else { return Err(api(format!("[EXTENSION] no index {name}"))) };
        for made in self.made_of_template(&table, name) {
            self.drop_index(&made)?;
        }
        self.set_template(&table, name, None)
    }

    /// The template `name` of `table` given `options`, or taken away if
    /// none.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the table is not partitioned.
    pub(in crate::db) fn set_template(&mut self, table: &QualifiedName, name: &str, options: Option<Vec<(String, Value)>>) -> Result<()> {
        let Some(PartitionDesc { table, key: Some(mut key), of }) = self.catalog.partitioning().get(table).cloned() else {
            return Err(api(format!("[EXTENSION] {table} is not partitioned")));
        };
        match options {
            Some(options) => key.templates.iter_mut().filter(|t| t.name == name).for_each(|t| t.options.clone_from(&options)),
            None => key.templates.retain(|t| t.name != name),
        }
        let mut catalog = self.catalog.duplicate();
        catalog.partitioning_mut().put(PartitionDesc { table, key: Some(key), of });
        self.catalog = std::sync::Arc::from(catalog);
        Ok(())
    }
}

impl Database {
    /// DROP INDEX of the template `name`, of `table`: each index it made
    /// dropped, then it, logged, under the table's exclusive lock and each
    /// partition's.
    ///
    /// # Errors
    ///
    /// As `drop_index`.
    pub(in crate::db) fn drop_template_index(&self, (table, name): (&QualifiedName, &str)) -> Result<()> {
        let current = self.snapshot()?;
        let tx = current.begin();
        current.acquire(&tx, table, LockMode::AccessExclusive)?;
        for leaf in current.catalog.partitioning().leaves(table) {
            current.acquire(&tx, &leaf, LockMode::AccessExclusive)?;
        }
        let _gate = current.tx.gate.enter();
        self.change(|st| {
            st.drop_template(name)?;
            st.log_definition(Some(&tx), Change::DropIndex { name: name.into() })
        })
    }

    /// ALTER INDEX of the template `name`, of `table`: each index it made
    /// altered as `altered_index` alters it, each logged, then it given the
    /// options, logged.
    ///
    /// # Errors
    ///
    /// As `altered_index`.
    pub(in crate::db) fn alter_template(
        &self,
        (table, name): (&QualifiedName, &str),
        given: &[(String, Option<Value>)],
        rebuilt: bool,
    ) -> Result<()> {
        let current = self.snapshot()?;
        for made in current.made_of_template(table, name) {
            self.altered_index(&made, given, rebuilt)?;
        }
        let Some((_, t)) = current.template(name) else { return Err(api(format!("[EXTENSION] no index {name}"))) };
        let declared = current.index_options_of(&t.method);
        let mut options = t.options.clone();
        for (option, value) in given {
            options = chosen(&t.method, (&declared, &options), (option, value.as_ref()))?;
        }
        let tx = current.begin();
        current.acquire(&tx, table, LockMode::AccessExclusive)?;
        let _gate = current.tx.gate.enter();
        self.change(|st| {
            st.set_template(table, name, Some(options.clone()))?;
            st.log_definition(Some(&tx), Change::AlterIndex { name: name.to_owned(), options: options.clone() })
        })
    }
}
