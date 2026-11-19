//! \[EXTENSION\] The partitions a read must read (RULES.md D156): of each
//! level of the tree, the partitions whose bounds may hold a row meeting
//! the read's conjuncts, as `partition/prune.rs` finds what they say of
//! the key. Where every part is restricted to a few values, each of their
//! combinations is routed, so a partition, the DEFAULT among them, is read
//! only if one routes to it; otherwise the strategy judges each bound
//! (`PartitionStrategy::admits`), and the DEFAULT is always read. A tree
//! whose strategy is not registered is read whole.

use crate::binder::BoundWrite;
use crate::catalog::{EXPRESSION_PART, PartitionBound, QualifiedName};
use crate::db::State;
use crate::logical::{Cond, LogicalPlan, split_and};
use crate::partition::Restriction;
use crate::partition::prune::restrictions;
use crate::value::Value;

/// \[DESIGN\] The most combinations of restricted values routed one by one
/// at a level, beyond which the strategy judges the bounds instead.
const MOST_ROUTED: usize = 64;

impl State {
    /// The partitions holding rows of the partitioned table `table` that
    /// a read of its rows meeting `conds` must read, in the order of the
    /// tree; of a table not partitioned, itself.
    pub(crate) fn admitted(&self, table: &QualifiedName, conds: &[Cond]) -> Vec<QualifiedName> {
        let p = self.catalog.partitioning();
        if !p.is_partitioned(table) {
            return vec![table.clone()];
        }
        let mut out = Vec::new();
        for child in self.admitted_children(table, conds) {
            if p.is_partitioned(&child) {
                out.extend(self.admitted(&child, conds));
            } else {
                out.push(child);
            }
        }
        out
    }

    /// The partitions of `table`, one level down, that a read meeting
    /// `conds` must read.
    fn admitted_children(&self, table: &QualifiedName, conds: &[Cond]) -> Vec<QualifiedName> {
        let children: Vec<(QualifiedName, PartitionBound)> = self
            .catalog
            .partitioning()
            .children(table)
            .into_iter()
            .filter_map(|d| d.of.as_ref().map(|o| (d.table.clone(), o.bound.clone())))
            .collect();
        let all = || children.iter().map(|(t, _)| t.clone()).collect();
        let Ok(Some(key)) = self.partition_key(table) else { return all() };
        let parts: Vec<Option<usize>> = key.key.columns.iter().map(|&c| (c != EXPRESSION_PART).then_some(c)).collect();
        let restricted = restrictions(conds, &parts, &key.types);
        if let Some(keys) = combinations(&restricted) {
            let bounds: Vec<&PartitionBound> = children.iter().map(|(_, b)| b).filter(|b| !matches!(b, PartitionBound::Default)).collect();
            let named: Vec<&QualifiedName> =
                children.iter().filter(|(_, b)| !matches!(b, PartitionBound::Default)).map(|(t, _)| t).collect();
            let default = children.iter().find(|(_, b)| matches!(b, PartitionBound::Default)).map(|(t, _)| t);
            let mut out: Vec<QualifiedName> = Vec::new();
            for k in keys {
                let Ok(found) = key.strategy.route(&bounds, &k) else { return all() };
                let to = found.and_then(|i| named.get(i).copied()).or(default);
                if let Some(t) = to.filter(|t| !out.contains(t)) {
                    out.push(t.clone());
                }
            }
            return children.iter().map(|(t, _)| t).filter(|t| out.contains(t)).cloned().collect();
        }
        children
            .iter()
            .filter(|(_, b)| matches!(b, PartitionBound::Default) || key.strategy.admits(b, &restricted).unwrap_or(true))
            .map(|(t, _)| t.clone())
            .collect()
    }
}

/// The partitions holding rows of the partitioned table `w` writes that
/// its search condition admits, each written in turn; none of a table not
/// partitioned.
pub(crate) fn written_members(st: &State, w: &BoundWrite) -> Vec<QualifiedName> {
    if !st.catalog.partitioning().is_partitioned(&w.table) {
        return Vec::new();
    }
    let mut conds = Vec::new();
    if let LogicalPlan::Filter { cond, .. } = &w.rows {
        split_and(cond.clone(), &mut conds);
    }
    st.admitted(&w.table, &conds)
}

/// Every key the restrictions allow, if each part is restricted to values
/// or null and they are at most `MOST_ROUTED`.
fn combinations(restricted: &[Option<Restriction>]) -> Option<Vec<Vec<Value>>> {
    let mut keys: Vec<Vec<Value>> = vec![Vec::new()];
    for r in restricted {
        let values = match r.as_ref()? {
            Restriction::Equal(values) => values.clone(),
            Restriction::Null => vec![Value::Null],
            Restriction::Range { .. } => return None,
        };
        if keys.len().saturating_mul(values.len()) > MOST_ROUTED {
            return None;
        }
        keys = keys.iter().flat_map(|k| values.iter().map(move |v| k.iter().cloned().chain([v.clone()]).collect())).collect();
    }
    Some(keys)
}
