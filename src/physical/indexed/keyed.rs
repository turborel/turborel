//! \[DESIGN\] The probes an index join may make of a relation by the
//! columns its joins equate (RULES.md D1297), taken from the relation's
//! table's indexes. Of each index, the columns the joins equate are taken
//! in the order of its key, and each first run of them is one probe,
//! priced as `probing` prices a probe of those columns, with the constants
//! of the relation's filter beside them as a point path takes them: a key
//! wholly covered finds one row, a prefix the rows of the prefix. Each
//! column is a probe alone too, as an index may be probed by a column
//! after its key's first (`indexed/expand.rs`). A join probes by the
//! cheapest of those whose columns are all among its keys
//! (`enumerate/model.rs`), so a key of several columns is priced as one
//! probe of them all, not as a probe of one of its columns.

use super::read_table;
use crate::logical::LogicalPlan;
use crate::physical::PhysicalPlanner;
use crate::physical::estimate::Known;
use crate::value::DataType;

/// A probe of a relation by the columns of the chain it names, and its cost.
pub(in crate::physical) type KeyProbe = (Vec<usize>, f64);

impl PhysicalPlanner<'_> {
    /// The probes of `right` by its columns `keyed`, each a column of the
    /// chain and its place in `right`: each alone, and of each index of its
    /// table, each first run of those its key holds, in its key's order.
    pub(in crate::physical) fn key_probes(&self, right: &LogicalPlan, keyed: &[(usize, usize)]) -> Vec<KeyProbe> {
        let any = |_: usize, _: &DataType| true;
        let priced = |run: &[(usize, usize)]| {
            let pairs: Vec<(usize, usize)> = run.iter().enumerate().map(|(left, &(_, at))| (left, at)).collect();
            let probing = self.probing(right, &pairs, &any)?;
            Some((run.iter().map(|&(column, _)| column).collect::<Vec<usize>>(), probing.cost))
        };
        let mut probes: Vec<KeyProbe> = keyed.iter().filter_map(|&key| priced(&[key])).collect();
        let Some(read) = read_table(right) else { return probes };
        let column_of = |at: usize| read.through.as_ref().map_or(Some(at), |(columns, _)| columns.get(at).copied());
        for ix in self.db.catalog.indexes(read.table) {
            let held = |c: usize| keyed.iter().copied().find(|&(_, at)| column_of(at) == Some(c));
            let run: Vec<(usize, usize)> = ix.columns.iter().filter_map(|&c| held(c)).collect();
            probes.extend((2..=run.len()).filter_map(|len| run.get(..len).and_then(priced)));
        }
        probes
    }
}

/// \[DESIGN\] (RULES.md D1297) The values of the columns `probed` of a
/// table of `rows` rows together, as `known`'s statistics count them,
/// else each column's multiplied: of two or more, whose guesses multiply
/// past any table, at most the rows; one column's guess stands, as a probe
/// of one value may find no row.
pub(super) fn joint(known: &Known<'_>, probed: &[usize], rows: f64) -> f64 {
    let distinct = |c: &usize| known.stats.and_then(|s| s.distinct(*c)).map_or(1.0_f64 / known.cost.equal_selectivity, |d| d.max(1.0_f64));
    let product = || {
        let guessed = probed.iter().map(distinct).product::<f64>();
        if probed.len() > 1 { guessed.min(rows) } else { guessed }
    };
    known.stats.and_then(|s| s.group_distinct(probed)).unwrap_or_else(product).max(1.0_f64)
}
