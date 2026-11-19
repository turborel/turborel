//! \[EXTENSION\] The read of a partitioned table (RULES.md D156): of each
//! partition its conjuncts admit (`State::admitted`), its own cheapest
//! path, by its own indexes and statistics. Of a read asked for in an
//! order, each partition's rows come in it, by an ordered index or a sort
//! of their own, and are merged (`Plan::MergeAppend`), so a LIMIT over
//! them reads few of each; otherwise the paths are appended in the order
//! of the tree. A read of no partition reads the partitioned table's own
//! storage, which holds no rows. A read of each row's id, of an UPDATE or
//! DELETE, is of the partitioned table alone: its statement writes each
//! partition in turn (`Session::searched`).
//!
//! A global index of the table (RULES.md D157) is a path too, when the
//! conjuncts fix each of its columns. One probe finds the one row of the
//! key, in whichever partition it is, read from there. It is taken when
//! it costs less than the partitions' paths.

use super::super::estimate::Known;
use super::super::indexed::access_cost;
use super::super::kept::point_probe;
use super::super::probe::{comparisons, served_by};
use super::super::{Built, PhysicalPlanner, Plan, TableRead};
use super::filter::{Filtered, filtered};
use super::predicate::{answerable, supports};
use super::{Probed, scans};
use crate::catalog::is_global_index;
use crate::logical::{Cond, SortKey, render_keys};
use crate::storage::cost_of;

impl PhysicalPlanner<'_> {
    /// The read of `read`'s table, partitioned, of the rows meeting
    /// `conds`; none of a table not partitioned, or a read of ids.
    pub(super) fn partitioned(&self, read: TableRead<'_>, conds: &[Cond], want: &[SortKey]) -> Option<Built> {
        if read.located || !self.db.catalog.partitioning().is_partitioned(read.table) {
            return None;
        }
        let mut each: Vec<Built> = Vec::new();
        for leaf in &self.db.admitted(read.table, conds) {
            let Ok(storage) = self.db.storage_of(leaf) else { continue };
            each.push(self.read_by(TableRead { table: leaf, ..read }, conds.to_vec(), (want, None), &**storage).0);
        }
        let spent: Option<f64> = each.iter().map(|b| access_cost(&b.plan)).sum();
        if let Some((global, cost)) = self.global_read(read, conds)
            && spent.is_none_or(|spent| cost < spent)
        {
            return Some(global);
        }
        if each.len() > 1 && !want.is_empty() {
            let detail = read.shown.text(|names| format!("MergeAppend {}", render_keys(want, names)));
            let inputs = each.into_iter().map(|b| if b.ordered { b.plan } else { sorted(b.plan, want, read) }).collect();
            return Some(Built { plan: Plan::MergeAppend { inputs, keys: want.to_vec(), detail }, ordered: true });
        }
        let mut built: Option<Built> = None;
        for one in each.into_iter().rev() {
            built = Some(match built {
                None => one,
                Some(after) => Built::unordered(Plan::Append { left: Box::new(one.plan), right: Box::new(after.plan) }),
            });
        }
        built
    }
}

impl PhysicalPlanner<'_> {
    /// The read of `read`'s table, partitioned, by a probe of a global
    /// index of it whose every column `conds` fix, and its cost; none if
    /// no such index is there.
    fn global_read(&self, read: TableRead<'_>, conds: &[Cond]) -> Option<(Built, f64)> {
        let (db, cost) = (self.db, self.db.cost());
        let desc = db.catalog.table(read.table)?;
        let known = Known { stats: None, cost: &cost };
        let leaves: Vec<_> = db.catalog.partitioning().leaves(read.table).iter().filter_map(|l| db.storage_of(l).ok()).cloned().collect();
        let rows = leaves.iter().map(|s| s.row_count()).fold(0, usize::saturating_add);
        let (fetch, miss) = leaves.first().map_or((crate::storage::DEFAULT_FETCH_COST, 0.0_f64), |s| (s.fetch_cost(), s.miss_cost()));
        let (compared, _) = comparisons(conds, known, self.values);
        let usable = |ix: &&crate::catalog::IndexDesc| is_global_index(&ix.name) && !db.building.contains(&ix.name);
        db.catalog.indexes(read.table).into_iter().filter(usable).find_map(|ix| {
            let (kind, found, types) = (db.methods.get(&ix.method)?, db.index_of(&ix.name).ok()?, ix.key_types(desc)?);
            let served = served_by(&compared, ix);
            let (probe, slots, used) = point_probe(answerable(conds), &served, (&ix.columns, &types))?;
            if !supports(&**kind, &probe, (&types, false)) {
                return None;
            }
            // Priced as a point path of a table's own unique index is.
            let descent = cost.index_descent * cost_of(rows.saturating_add(1)).log2();
            let pages = found.probe_pages(&probe).map_or(descent, |pages| cost.index_descent * pages);
            let total = pages + found.probe_missed().unwrap_or(0.0_f64) * miss + fetch;
            let (index, method, columns) = (ix.name.clone(), ix.method.clone(), ix.columns.clone());
            let mut probed =
                Probed { index, method, columns, probes: vec![probe], slots, used, loose: false, covering: false, decided: None };
            let mut kept = conds.to_vec();
            let answered = probed.answered(&mut kept);
            let plan = scans::index_scans(read, probed, answered, (1.0_f64, total, false));
            let with = Filtered { shown: read.shown, known, ordered: true, read: 1.0_f64, most: 1.0_f64, note: "", windowed: None };
            // One row at most, in any order asked for.
            let built = Built { ordered: true, ..filtered(plan, kept, &with) };
            Some((built, total))
        })
    }
}

/// `plan`, one partition's read, sorted in the order of `want`.
fn sorted(plan: Plan, want: &[SortKey], read: TableRead<'_>) -> Plan {
    Plan::Sort { keys: want.to_vec(), detail: read.shown.text(|names| render_keys(want, names)), input: Box::new(plan), most: None }
}
