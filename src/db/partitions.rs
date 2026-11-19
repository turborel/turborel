//! \[EXTENSION\] Partitioned tables in the database (RULES.md D153 to
//! D159): the strategies registered beside the built-in ones, and what a
//! statement asks of a table's tree: its key, bound over the table's row,
//! and its strategy. The definitions are made in `db/partitions/define.rs`,
//! attached and detached in `db/partitions/attach.rs`, and rows routed as
//! they are written in `db/partitions/write.rs`.
//!
//! The registry is shared, not copied with each change of the database's
//! state, as the rankings' is (`db/rankings.rs`): a strategy registered
//! after a table of it was defined is found as the table is next used.

mod attach;
pub(super) use cursor::{leaf_plan, partition_members};
mod cursor;
pub(super) use attach::validate_attached;
mod define;
pub(crate) mod dispatch;
mod drop;
pub(crate) mod global;
mod indexes;
mod prune;
mod ranked;
mod templates;
pub(super) use prune::written_members;
pub(crate) mod write;
pub(super) use write::move_row;

use std::sync::{Arc, PoisonError, RwLock};

use super::{Database, State};
use crate::catalog::{EXPRESSION_PART, IndexDesc, KeyExpression, PartitionKey, QualifiedName, TableDesc};
use crate::diagnostics::Extension;
use crate::error::{Code, Result, SqlError};
use crate::lexer::authorization_identifier;
use crate::partition::{PartitionStrategy, builtin};
use crate::value::DataType;

/// The strategies registered, beside the built-in, and the routes made of
/// the partitioned tables written (`db/partitions/write.rs`).
#[derive(Clone, Default)]
pub(crate) struct SharedStrategies {
    registered: Arc<RwLock<Vec<Arc<dyn PartitionStrategy>>>>,
    pub(crate) routes: Arc<write::Routes>,
}

impl SharedStrategies {
    /// The strategy named `name`: a built-in one, else one registered.
    pub(crate) fn named(&self, name: &str) -> Option<Arc<dyn PartitionStrategy>> {
        if let Some(s) = builtin().into_iter().find(|s| s.name() == name) {
            return Some(s);
        }
        self.registered.read().unwrap_or_else(PoisonError::into_inner).iter().find(|s| s.name() == name).cloned()
    }

    /// Adds `strategy`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its name is built in or registered.
    fn add(&self, strategy: Arc<dyn PartitionStrategy>) -> Result<()> {
        if self.named(strategy.name()).is_some() {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] a partition strategy {} is registered already", strategy.name())));
        }
        self.registered.write().unwrap_or_else(PoisonError::into_inner).push(strategy);
        Ok(())
    }
}

impl Database {
    /// \[EXTENSION\] Registers a partition strategy, which PARTITION BY then
    /// names (RULES.md D154). Its contract is `PartitionStrategy`'s. A
    /// table of a strategy not registered keeps its rows and is read, but
    /// no row is routed to it and no read pruned until it is.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is no regular identifier in upper case, or
    /// a strategy of its name is built in or registered already.
    pub fn register_partition_strategy(&self, strategy: Box<dyn PartitionStrategy>) -> Result<()> {
        let strategy: Arc<dyn PartitionStrategy> = Arc::from(strategy);
        let name = strategy.name().to_owned();
        if authorization_identifier(&name).ok().as_deref() != Some(name.as_str()) {
            let msg = format!("[DESIGN] a partition strategy is named by a regular identifier in upper case, not {name}");
            return self.registered((Extension::PartitionStrategy, &name), Err(SqlError::new(Code::Api, msg)));
        }
        let done = self.snapshot().and_then(|st| st.strategies.add(strategy));
        self.registered((Extension::PartitionStrategy, &name), done)
    }
}

/// A partitioned table's key, bound over its row: what routes a row and
/// what its parts' types are.
pub(crate) struct BoundPartitionKey {
    /// The key as an index's key is, its parts' values of a row
    /// `computed_key`'s (`catalog/keys.rs`).
    pub(crate) key: IndexDesc,
    /// The types of its parts.
    pub(crate) types: Vec<DataType>,
    /// Its strategy.
    pub(crate) strategy: Arc<dyn PartitionStrategy>,
}

impl State {
    /// The strategy named `name`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if none is registered of the name.
    pub(crate) fn strategy(&self, name: &str) -> Result<Arc<dyn PartitionStrategy>> {
        let brought = || match self.extension_object(crate::plugin::ObjectKind::Partition, name) {
            Some(crate::plugin::Bridged::Partition(p)) => {
                let strategy: Arc<dyn PartitionStrategy> = p;
                Some(strategy)
            }
            Some(_) | None => None,
        };
        self.strategies.named(name).or_else(brought).ok_or_else(|| {
            SqlError::new(Code::SyntaxRule, format!("[EXTENSION] no partition strategy {name} is registered (RULES.md D154)"))
        })
    }

    /// The key of the partitioned table `table`, bound over its row, if it
    /// is partitioned.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its strategy is not registered; a key expression's
    /// binding errors.
    pub(crate) fn partition_key(&self, table: &QualifiedName) -> Result<Option<BoundPartitionKey>> {
        let Some(key) = self.catalog.partitioning().key(table).cloned() else { return Ok(None) };
        let Some(desc) = self.catalog.table(table).cloned() else { return Ok(None) };
        self.bind_partition_key(&desc, &key).map(Some)
    }

    /// `key`, of the table `desc`, bound over its row.
    ///
    /// # Errors
    ///
    /// As `partition_key`.
    pub(crate) fn bind_partition_key(&self, desc: &TableDesc, key: &PartitionKey) -> Result<BoundPartitionKey> {
        let strategy = self.strategy(&key.strategy)?;
        let ix = key_desc(&desc.name, key);
        let ix = self.bound_keys(ix, desc)?;
        let types = ix
            .key_types(desc)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the partition key of {} names no column", desc.name)))?;
        Ok(BoundPartitionKey { key: ix, types, strategy })
    }
}

/// The partition key `key` of `table` as an index's key, unbound, so
/// computed of a row as an index's is, expressions and all (RULES.md D92).
pub(crate) fn key_desc(table: &QualifiedName, key: &PartitionKey) -> IndexDesc {
    let expressions = key.expressions.iter().map(|(part, text)| KeyExpression::unbound(*part, text.clone())).collect();
    let mut columns = key.columns.clone();
    for (part, _) in &key.expressions {
        if let Some(c) = columns.get_mut(*part) {
            *c = EXPRESSION_PART;
        }
    }
    IndexDesc {
        name: format!("{table} partition key"),
        table: table.clone(),
        columns,
        method: String::new(),
        unique: false,
        options: Vec::new(),
        collations: Vec::new(),
        nulls: true,
        expressions,
    }
}
