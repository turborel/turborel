//! \[DESIGN\] A conjunction over a plan's columns, estimated a table at a
//! time (RULES.md D370, D382): the conjuncts whose columns all come from
//! one scan are one conjunction of that table's columns, estimated
//! together by its statistics (`estimate::conjunction`), so conjuncts
//! that go together are not taken as independent; each other conjunct,
//! an equality of two relations' columns among them, by itself
//! (`Estimates::share`). Tables are taken as independent of each other.

use super::Estimates;
use super::joins::columns_of;
use super::lineage::Over;
use crate::catalog::QualifiedName;
use crate::logical::{Cond, LogicalPlan, map_cond_columns};
use crate::physical::estimate::{Known, conjunction};

/// The conjuncts of one scan, of its table's columns.
struct OfScan<'p> {
    scan: &'p LogicalPlan,
    table: &'p QualifiedName,
    conds: Vec<Cond>,
}

impl Estimates<'_> {
    /// The fraction of the rows the conjuncts `conjuncts`, over `over`'s
    /// columns, keep: of each scan, its conjuncts together; of the rest,
    /// each alone.
    pub(super) fn conjunction(&self, conjuncts: &[Cond], over: Over<'_>) -> f64 {
        let mut scans: Vec<OfScan<'_>> = Vec::new();
        let mut kept = 1.0_f64;
        for c in conjuncts {
            match local(c, over) {
                Some((scan, table, local)) => match scans.iter_mut().find(|s| std::ptr::eq(s.scan, scan)) {
                    Some(of) => of.conds.push(local),
                    None => scans.push(OfScan { scan, table, conds: vec![local] }),
                },
                None => kept *= self.share(c, over),
            }
        }
        for of in &scans {
            let stats = self.statistics(of.table);
            let share = conjunction(&of.conds, Known { stats: stats.as_deref(), cost: self.cost() });
            kept *= self.width_capped(of.table, &of.conds, self.past_capped(of.table, &of.conds, share));
        }
        kept
    }
}

/// The one scan every column of `c`, over `over`'s columns, comes from,
/// its table, and `c` of that table's columns, if `c` reads columns of
/// one scan alone.
fn local<'p>(c: &Cond, over: Over<'p>) -> Option<(&'p LogicalPlan, &'p QualifiedName, Cond)> {
    let columns = columns_of(c);
    let mut places = Vec::with_capacity(columns.len());
    let mut first: Option<(&'p LogicalPlan, &'p QualifiedName)> = None;
    for &column in &columns {
        let found = over.origin(column)?;
        if first.is_some_and(|(scan, _)| !std::ptr::eq(scan, found.scan)) {
            return None;
        }
        first = Some((found.scan, found.table));
        places.push((column, found.column));
    }
    let (scan, table) = first?;
    let mapped = map_cond_columns(c.clone(), &|col| places.iter().find(|&&(at, _)| at == col).map_or(col, |&(_, place)| place));
    Some((scan, table, mapped))
}
