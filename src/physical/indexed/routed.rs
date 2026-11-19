//! \[EXTENSION\] Index nested-loop joins into a partitioned table (RULES.md
//! D156). Its rows are its partitions', so the indexes a probe reads are
//! its global ones (D157), each one probe of every partition's rows, and
//! those of its templates and its ancestors' (D158), which each partition
//! has, a probe of each partition read. Of a probe whose values fix every
//! column of every partition key of the tree, each probe is routed, as it
//! is made, to the one partition that may hold its rows, so run-time
//! pruning reads one index, not every partition's (`exec/join/indexed.rs`).

use std::sync::Arc;

use super::super::PhysicalPlanner;
use crate::catalog::{EXPRESSION_PART, IndexDesc, QualifiedName, is_global_index, template_of};
use crate::index::{Index, Probe};
use crate::storage::{TableStorage, cost_of};
use crate::value::{DataType, Value};

/// An index a join's probes may read: of a table its own, of a partitioned
/// table a global index or a template's, priced by one of its partitions'.
pub(in crate::physical) struct Candidate {
    pub(super) ix: IndexDesc,
    pub(super) found: Arc<dyn Index>,
    /// Of a template's, how its partitions are probed.
    pub(super) routing: Option<Routing>,
}

/// The partitions a probe of a template's index reads.
pub(in crate::physical) struct Routing {
    /// How many partitions hold rows.
    leaves: usize,
    /// The columns of every partition key of the tree, if none is an
    /// expression.
    keys: Option<Vec<usize>>,
}

impl Candidate {
    /// The pages a probe of `found` by values of the first `used` of the
    /// key parts' types `types` reads, asked of the index by a sample value of each; of a
    /// reverse probe or one of a type without samples, about a descent of
    /// a tree of `rows` rows.
    pub(super) fn pages(found: &dyn Index, (types, used): (&[DataType], usize), reverse: bool, rows: f64) -> f64 {
        let sample: Option<Vec<Value>> = types.iter().take(used).map(super::sample).filter(|_| !reverse).collect();
        sample.and_then(|values| found.probe_pages(&Probe::Prefix { equal: values, then: None })).unwrap_or_else(|| (rows + 1.0_f64).log2())
    }
}

impl Routing {
    /// The partitions a probe of the table's columns `probed` reads: one if
    /// they fix every partition key, else every one; and what EXPLAIN
    /// tells of it after the index's method.
    pub(super) fn probes(&self, probed: &[usize]) -> (f64, String) {
        if self.keys.as_ref().is_some_and(|keys| keys.iter().all(|c| probed.contains(c))) {
            return (1.0_f64, ", each partition's, each probe routed to one".to_owned());
        }
        (cost_of(self.leaves), format!(", each partition's, each probe of {}", self.leaves))
    }
}

impl PhysicalPlanner<'_> {
    /// The indexes a probe of the partitioned table `table` may read, and
    /// the storage of a partition of it, which prices its fetches; none if
    /// no partition holds rows.
    pub(super) fn routed_candidates(&self, table: &QualifiedName) -> Option<(Arc<dyn TableStorage>, Vec<Candidate>)> {
        let (db, p) = (self.db, self.db.catalog.partitioning());
        let leaves = p.leaves(table);
        let storage = Arc::clone(db.storage_of(leaves.first()?).ok()?);
        let ready = |ix: &&IndexDesc| !db.building.contains(&ix.name);
        let mut out: Vec<Candidate> = Vec::new();
        for ix in db.catalog.indexes(table).into_iter().filter(|ix| is_global_index(&ix.name)).filter(ready) {
            if let Ok(found) = db.index_of(&ix.name) {
                out.push(Candidate { ix: ix.clone(), found: Arc::clone(found), routing: None });
            }
        }
        let tables = std::iter::once(table.clone()).chain(p.descendants(table)).filter(|t| p.is_partitioned(t));
        let columns: Vec<usize> = tables.filter_map(|t| p.key(&t).cloned()).flat_map(|k| k.columns).collect();
        let keys = (!columns.contains(&EXPRESSION_PART)).then_some(columns);
        let owners = std::iter::once(table.clone()).chain(p.ancestors(table));
        for t in owners.filter_map(|o| p.key(&o).cloned()).flat_map(|k| k.templates) {
            let each: Option<Vec<&IndexDesc>> = leaves
                .iter()
                .map(|leaf| db.catalog.indexes(leaf).into_iter().filter(ready).find(|ix| template_of(&ix.name) == Some(t.name.as_str())))
                .collect();
            let Some(first) = each.and_then(|e| e.first().copied()) else { continue };
            let Ok(found) = db.index_of(&first.name) else { continue };
            let ix = IndexDesc { name: t.name.clone(), table: table.clone(), ..first.clone() };
            let routing = Some(Routing { leaves: leaves.len(), keys: keys.clone() });
            out.push(Candidate { ix, found: Arc::clone(found), routing });
        }
        Some((storage, out))
    }
}
