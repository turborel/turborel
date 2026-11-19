//! \[EXTENSION\] The probes of an index join into a partitioned table
//! (`physical/indexed/routed.rs`, RULES.md D156): of a global index, its
//! rows read from the partitions its entries name (D157); of a template's,
//! each probe routed, as its values are bound, to the partitions that may
//! hold its rows, as a read fixing those values is pruned
//! (`State::admitted`), each one's index read and its rows' ids packed
//! with its id, so they are read from it (`Dispatched`).

use std::collections::HashMap;
use std::sync::Arc;

use crate::catalog::{IndexDesc, KeyShape, QualifiedName, is_global_index, template_of};
use crate::db::State;
use crate::db::partitions::dispatch::Dispatched;
use crate::db::partitions::global::packed;
use crate::error::{Code, Result, SqlError};
use crate::index::{Entries, Index, KeyOrder, Probe};
use crate::logical::{Cond, Expr};
use crate::storage::TableStorage;
use crate::value::{CompOp, Value};

/// What an index join reads opened: the table's rows, the index, the shape
/// of its keys, and of a template's, the routes of its probes.
pub(super) type Opened = (Arc<dyn TableStorage>, Arc<dyn Index>, KeyShape, Option<Routes>);

/// The partitions of a partitioned table a template's index is probed in.
pub(super) struct Routes {
    db: Arc<State>,
    table: QualifiedName,
    /// The table's columns of the index's key, in its order.
    columns: Vec<usize>,
    /// Each partition holding rows, its id and its index of the template.
    leaves: HashMap<QualifiedName, (u64, Arc<dyn Index>)>,
}

/// What an index join of the partitioned table `table` by `index` reads:
/// the rows of its partitions, the index probed, or one of them, its key's
/// shape, and of a template's index, the routes of its probes; none of a
/// table not partitioned.
///
/// # Errors
///
/// SQLCODE -910 if a partition lacks the template's index.
pub(super) fn opened(db: &Arc<State>, table: &QualifiedName, index: &str) -> Result<Option<Opened>> {
    let p = db.catalog.partitioning();
    if !p.is_partitioned(table) {
        return Ok(None);
    }
    let storage: Arc<dyn TableStorage> = Arc::new(Dispatched::of(db, table));
    if is_global_index(index) {
        let ix = db.catalog.index(index).ok_or_else(|| missing(index))?;
        return Ok(Some((storage, Arc::clone(db.index_of(index)?), ix.key_shape(), None)));
    }
    let mut leaves = HashMap::new();
    let mut shape: Option<IndexDesc> = None;
    for leaf in p.leaves(table) {
        let of = p.of(&leaf).ok_or_else(|| missing(index))?;
        let ix = db.catalog.indexes(&leaf).into_iter().find(|ix| template_of(&ix.name) == Some(index)).ok_or_else(|| missing(index))?;
        leaves.insert(leaf.clone(), (of.id, Arc::clone(db.index_of(&ix.name)?)));
        shape.get_or_insert_with(|| ix.clone());
    }
    let shape = shape.ok_or_else(|| missing(index))?;
    let found = leaves.values().next().map(|(_, found)| Arc::clone(found)).ok_or_else(|| missing(index))?;
    let routes = Routes { db: Arc::clone(db), table: table.clone(), columns: shape.columns.clone(), leaves };
    Ok(Some((storage, found, shape.key_shape(), Some(routes))))
}

/// The error of an index a partition lacks.
fn missing(index: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] a partition has no index of {index}"))
}

impl Routes {
    /// The entries `probe` finds in the index of each partition its values
    /// may be in, each of its row's id packed with the partition's.
    pub(super) fn search(&self, probe: &Probe) -> Entries {
        let fixed = self.fixed(probe);
        let routed = self.db.admitted(&self.table, &fixed);
        let mut each: Vec<Entries> = Vec::new();
        for leaf in &routed {
            let Some((partition, index)) = self.leaves.get(leaf) else { continue };
            let partition = *partition;
            let found = Arc::clone(index).search(probe, KeyOrder::Ascending);
            each.push(Box::new(found.map(move |entry| entry.and_then(|(key, id)| Ok((key, packed(partition, id)?))))));
        }
        Box::new(each.into_iter().flatten())
    }

    /// The equalities `probe` fixes of the table's columns, as conjuncts
    /// a read is pruned by.
    fn fixed(&self, probe: &Probe) -> Vec<Cond> {
        let equal = |at: usize, value: &Value| {
            self.columns.get(at).map(|&c| Cond::Compare(Expr::Column(c), CompOp::Eq, Expr::Const(value.clone())))
        };
        match probe {
            Probe::Compare { op: CompOp::Eq, value } => equal(0, value).into_iter().collect(),
            Probe::Prefix { equal: values, .. } | Probe::Range { equal: values, .. } => {
                values.iter().enumerate().filter_map(|(at, v)| equal(at, v)).collect()
            }
            Probe::Columns(compared) => compared.iter().filter(|c| c.op == CompOp::Eq).filter_map(|c| equal(c.column, &c.value)).collect(),
            Probe::Compare { .. } | Probe::All | Probe::Predicate { .. } | Probe::Xml(_) => Vec::new(),
        }
    }
}
