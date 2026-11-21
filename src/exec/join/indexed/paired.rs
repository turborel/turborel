//! \[DESIGN\] A geometry join that descends the R-trees of both its
//! tables together (RULES.md D138), pairing the entries whose envelopes
//! meet, rather than probing one tree for each outer row. Each pair's
//! rows are fetched as the statement sees them, their versions' keys
//! rechecked as a probe's are, and the pair kept only if the outer
//! operand's filter is true of its outer row and the whole condition of
//! the pair [X3.135-1986 §5.20 GR2b, §5.21 GR2]. Of a LEFT or FULL outer
//! join [ISO/IEC 9075:1992 §7.5 GR1 to GR5], the ids of the rows paired
//! are kept, and the outer table, then of a FULL join the probed one,
//! read again for the rows no pair kept. A tree that cannot be read so
//! leaves the join to its probes. The pairs come in the trees' order.
//!
//! A serializable read records the whole outer table and, of the probed
//! table, the hull of the outer tree's envelopes (D134): a row of either
//! that would pair with a row of the other meets that hull or is in the
//! outer table.

mod rest;

use std::sync::Arc;

use super::super::super::entries::Reading;
use super::super::super::{RowIter, Runner, broken, eval_cond, execute, failed};
use super::{IndexJoin, Probed, Step, preserved::Ids};
use crate::catalog::IndexDesc;
use crate::error::Result;
use crate::exec::Db;
use crate::index::{Key, KeyPairs, Probe, Using};
use crate::logical::{Cond, Truth};
use crate::physical::{Paired, Plan};
use crate::storage::{Condition, Region, Row, RowId, sized_row};
use crate::txn::checked;

/// The join of `paired`'s table with `join`'s by their trees, or, if
/// either has none, `join` driven by the rows of `left`.
pub(super) fn start(mut join: IndexJoin, (paired, left): (Paired, Plan), db: &Db) -> RowIter {
    let opened = Traversal::open(&join, paired, db);
    match opened {
        Ok(Some(traversal)) => Box::new(Driven { join, traversal, last: None, phase: Phase::Pairs }),
        Ok(None) => {
            join.left = execute(left, db, join.table.tx);
            Box::new(join)
        }
        Err(error) => failed(error),
    }
}

/// The outer table and the pairs read of the two trees.
struct Traversal {
    outer: Probed,
    filter: Option<Cond>,
    through: Option<Vec<usize>>,
    pairs: KeyPairs,
    /// Of an outer join, the outer rows paired.
    kept: Option<Ids>,
    columns: Vec<usize>,
}

impl Traversal {
    fn open(join: &IndexJoin, paired: Paired, db: &Db) -> Result<Option<Traversal>> {
        let Paired { table, index, filter, through, columns, located } = paired;
        let shape = db.catalog.index(&index).map(IndexDesc::key_shape);
        let (Some(ix), Ok(found)) = (shape, db.index_of(&index)) else { return Ok(None) };
        let (Some(outer_tree), Some(inner_tree)) = (found.envelopes(), join.table.index.envelopes()) else { return Ok(None) };
        let storage = Arc::clone(db.storage_of(&table)?);
        let tx = join.table.tx;
        // The pairs depend on every outer row, and on the probed rows
        // meeting the outer tree's hull (D134).
        storage.read_by_condition(tx, &Condition { columns: Vec::new(), probe: Probe::All })?;
        let hull: Vec<_> = outer_tree.hull()?.into_iter().collect();
        let inner = &join.table;
        match inner.ix.columns.as_slice() {
            [column] if !inner.ix.has_expressions() => inner.storage.read_by_region(tx, &Region::new(*column, hull))?,
            _ => inner.storage.read_by_condition(tx, &Condition { columns: Vec::new(), probe: Probe::All })?,
        }
        let stop = join.table.stop.clone();
        let pairs: KeyPairs = Box::new(checked(outer_tree.pairs(&inner_tree, &db.memory)?, stop.clone()));
        let reading = Reading::of(&ix, Some(&columns), found.key_projection());
        let outer = Probed { storage, index: Arc::clone(found), ix, reading, tx, stop, located, routes: None, using: Using::default() };
        let kept = if join.nulls.is_none() { None } else { Some(Ids::new(db)?) };
        Ok(Some(Traversal { outer, filter, through, pairs, kept, columns }))
    }

    /// The outer row of entry `(key, id)`, if the version read has the
    /// key and the operand's filter is true of it.
    fn outer_row(&mut self, (key, id): (Key, RowId), runner: &Runner) -> Result<Option<Row>> {
        let Some(row) = self.outer.row(Ok((key, id)))? else { return Ok(None) };
        self.operand(row, runner)
    }

    /// The operand's row of the table's row `row`, if its filter is true
    /// of it: of a projection, the columns it takes.
    fn operand(&self, row: Row, runner: &Runner) -> Result<Option<Row>> {
        let kept = match &self.filter {
            Some(filter) => eval_cond(filter, &row, runner)? == Truth::True,
            None => true,
        };
        if !kept {
            return Ok(None);
        }
        let Some(through) = &self.through else { return Ok(Some(row)) };
        let taken = through.iter().map(|&c| row.get(c).cloned().ok_or_else(|| broken("a traversal's outer column is missing")));
        sized_row(through.len(), taken).map(Some)
    }
}

/// Where the join is: pairing, then reading the outer table for the rows
/// no pair kept, then the probed table's.
enum Phase {
    Pairs,
    Outer(rest::Rest),
    Inner,
}

/// The join, its inner side's rows, condition and output `join`'s.
struct Driven {
    join: IndexJoin,
    traversal: Traversal,
    /// The outer entry last read and its row, if kept, as a leaf's entry
    /// is paired with each entry of the other leaf in turn.
    last: Option<((Key, RowId), Option<Row>)>,
    phase: Phase,
}

impl Driven {
    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            match &mut self.phase {
                Phase::Pairs => {
                    if let Some(row) = self.paired()? {
                        return Ok(Some(row));
                    }
                }
                Phase::Outer(rest) => {
                    if let Some(row) = rest.next(&self.traversal, &self.join)? {
                        return Ok(Some(row));
                    }
                    self.phase = Phase::Inner;
                }
                Phase::Inner => {
                    return match self.join.unmatched()? {
                        Step::Row(row) => Ok(Some(row)),
                        Step::ProbeEnd | Step::End => Ok(None),
                    };
                }
            }
        }
    }

    /// The next pair the condition is true of, or none once the pairs
    /// are read, the next phase then begun.
    fn paired(&mut self) -> Result<Option<Row>> {
        while let Some(pair) = self.traversal.pairs.next() {
            let (outer, inner) = pair?;
            let (outer_id, inner_id) = (outer.1, inner.1);
            let Some(l) = self.outer(outer)? else { continue };
            let Some(row) = self.join.paired(&l, Ok(inner))? else { continue };
            if let Some(kept) = self.traversal.kept.as_mut() {
                kept.add(outer_id)?;
            }
            if let Some(matched) = self.join.matched.as_mut() {
                matched.add(inner_id)?;
            }
            return Ok(Some(row));
        }
        self.phase = match self.traversal.kept.take() {
            Some(kept) => Phase::Outer(rest::Rest::new(kept)),
            None => Phase::Inner,
        };
        Ok(None)
    }

    /// The row of outer entry `entry`, read once for the pairs of it that
    /// follow one another, and the condition prepared for it.
    fn outer(&mut self, entry: (Key, RowId)) -> Result<Option<Row>> {
        if let Some((last, row)) = &self.last
            && *last == entry
        {
            return Ok(row.clone());
        }
        self.last = None;
        let row = self.traversal.outer_row(entry.clone(), &self.join.runner)?;
        if let Some(row) = &row {
            self.join.prepare(row)?;
        }
        self.last = Some((entry, row.clone()));
        Ok(row)
    }
}

impl Iterator for Driven {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}
