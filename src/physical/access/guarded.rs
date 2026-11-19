//! \[DESIGN\] RULES.md D15: the read of a table a filter is of: directly,
//! its access path; or over the table's read and a guard, read below the
//! guard.
//!
//! A conjunct that can raise an error, of one operand of a product or of
//! the operand an outer join gives nulls, is moved onto that operand over
//! a guard of the others, a row of no columns if each has a row
//! (`optimizer/raising.rs`, `optimizer/outer.rs`), so it is evaluated on
//! every row of the operand when they have rows, and on none when one has
//! none. Of an operand that reads a table, the conjuncts may choose an
//! index as a filter of the table's would (`access.rs`): a probe leaves out
//! only rows of which no conjunct is true or raises an error, the rows
//! D15 lets a WHERE clause's index skip. The index's rows are then the
//! product's operand, and every conjunct but those the index answers,
//! which raise none, the product's filter: each evaluated, as over the
//! guard, on the rows the index gives when the guard has a row, and on
//! none when it has none. A read of no index leaves the plan as it is.

use super::super::{Built, PhysicalPlanner, Plan, TableRead};
use super::{Filtered, Keyed, filtered, predicate, scans, xml};
use crate::index::Probe;
use crate::logical::{Cond, LogicalPlan, SortKey, split_and};
use crate::physical::estimate::Known;
use crate::storage::{TableStorage, cost_of};

impl PhysicalPlanner<'_> {
    /// The read of the table `cond` filters, as `input` reads it: directly,
    /// its access path; or, of a product of the table's read, filtered or
    /// not, and a guard of no columns, an index's read below the guard, if
    /// its conjuncts choose one. None of another.
    pub(in crate::physical) fn filtered_read(&self, cond: &Cond, input: &LogicalPlan, want: &[SortKey]) -> Option<Built> {
        let mut conds = Vec::new();
        split_and(cond.clone(), &mut conds);
        if let LogicalPlan::Scan { table, names, needed, located } = input {
            return Some(self.access(TableRead { table, shown: self.showing(names), needed, located: *located }, conds, want));
        }
        let LogicalPlan::Product { left, right: guard } = input else { return None };
        if guard.width() != 0 {
            return None;
        }
        let mut read = &**left;
        if let LogicalPlan::Filter { cond: own, input: below } = read {
            split_and(own.clone(), &mut conds);
            read = below;
        }
        let LogicalPlan::Scan { table, names, needed, located } = read else { return None };
        self.guarded_access(TableRead { table, shown: self.showing(names), needed, located: *located }, conds, (want, guard))
    }

    /// RULES.md D15: the table read by an index its conjuncts `conds`
    /// choose, below `guard`, a plan of no columns: a product of the
    /// index's rows and the guard's, under a filter of the conjuncts it
    /// does not answer (`filtered_read`); none if no index is chosen.
    pub(super) fn guarded_access(&self, read: TableRead<'_>, conds: Vec<Cond>, (want, guard): (&[SortKey], &LogicalPlan)) -> Option<Built> {
        // [EXTENSION] a partitioned table's partitions are read by `access`.
        if self.db.catalog.partitioning().is_partitioned(read.table) {
            return None;
        }
        let storage = self.db.storage_of(read.table).ok()?;
        let (built, indexed) = self.read_by(read, conds, (want, Some(guard)), &**storage);
        indexed.then_some(built)
    }

    /// The table of `storage` read by its cheapest path, under a filter of
    /// the conjuncts of `conds` it does not answer; the path, if it reads
    /// an index and `guard` is given, a product of it and the guard's plan.
    /// Whether it reads an index.
    pub(super) fn read_by(
        &self,
        read: TableRead<'_>,
        mut conds: Vec<Cond>,
        (want, guard): (&[SortKey], Option<&LogicalPlan>),
        storage: &dyn TableStorage,
    ) -> (Built, bool) {
        let rows = storage.row_count();
        // RULES.md D106: an implied filter is no condition of the rows read:
        // an index may find them by it, and no row is checked by it.
        let (implied, kept): (Vec<Cond>, Vec<Cond>) = conds.into_iter().partition(xml::implied);
        conds = kept;
        let best = self.best_path(read, (&conds, &implied), want);
        let indexed = best.index.is_some();
        // A scan's filter keeps its share of the table's rows, but no more
        // than the scan reads: the rows its zones or chunks may match.
        let (plan, (found, most)) = match best.index {
            None => {
                let order = best.order;
                let (columns, backward) = (read.needed.to_vec(), best.reverse);
                let plan = Plan::SeqScan {
                    table: read.table.clone(),
                    columns,
                    rows,
                    cost: best.cost,
                    order,
                    backward,
                    most: None,
                    located: read.located,
                };
                (plan, (cost_of(rows), best.est))
            }
            Some(mut probed) => {
                let (named, keyed) = (predicate::named(&probed.probes), Keyed::of_read(self.db, read));
                let answered = probed.answered(&mut conds);
                // A function lookup is priced as of every row; an XML lookup by
                // its index's count.
                let function = named && !matches!(probed.probes.first(), Some(Probe::Xml(_)));
                let found = if function { cost_of(rows) } else { best.est };
                (scans::index_scans(keyed.read(read), probed, answered, (best.est, best.cost, best.reverse)), (found, best.est))
            }
        };
        let plan = match guard.filter(|_| indexed) {
            Some(guard) => Plan::Product { left: Box::new(plan), right: Box::new(self.build_node(guard, &[]).plan) },
            None => plan,
        };
        let (cost, stats) = (self.db.cost(), self.db.statistics(read.table));
        let known = Known { stats: stats.as_deref(), cost: &cost };
        (
            filtered(
                plan,
                conds,
                &Filtered { shown: read.shown, known, ordered: best.ordered, read: found, most, note: &best.note, windowed: best.windowed },
            ),
            indexed,
        )
    }
}
