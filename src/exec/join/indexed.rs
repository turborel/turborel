//! The index nested-loop join (`physical/indexed.rs`, RULES.md D35): the
//! rows of a product its comparisons keep [X3.135-1986 §5.20 GR2b, §5.21
//! GR2], or of a LEFT outer join [ISO/IEC 9075:1992 §7.5], each row of
//! the left operand joined with the rows an index of the right's table
//! finds for its values, as they are found, those the rest of the
//! condition is true of kept; of a LEFT join, a left row none is kept of
//! with nulls. The left is read as it is pulled and nothing is held: its
//! rows' order is the result's, each row's matches in the index's. The
//! statement looks for a cancel before each probe and as each entry is
//! read (RULES.md D44).
//! An unordered spatial join can instead buffer a bounded outer input
//! and scan the table once when estimated cheaper (`indexed/adaptive.rs`).

use std::sync::Arc;

use super::super::entries::{Reading, fetch};
use super::super::project::Columns;
use super::super::read::Asked;
use super::super::{RowIter, Runner, Subqueries, broken, eval_cond, execute, failed};
use crate::catalog::{IndexDesc, KeyShape, QualifiedName};
use crate::db::State;
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::index::{Entries, Index, Key, KeyOrder};
use crate::logical::{Cond, Truth};
use crate::memory::MemoryBudget;
use crate::optimizer::prepared;
use crate::physical::{JoinProbe, Plan, Preserved, Spatial};
use crate::storage::{self, Row, RowId, TableStorage};
use crate::txn::{Stop, TxId, checked};
use crate::value::Value;

mod adaptive;
mod paired;
mod preserved;
mod refined;
mod routed;

use preserved::Matched;
use refined::Binding;
use routed::{Opened, Routes};

/// The rows of the index nested-loop join `plan`. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack (RULES.md
/// R-CF-6).
#[inline(never)]
pub(in crate::exec) fn indexed(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    projected(plan, None, db, tx)
}

/// Fuse a column projection into a join, preserving its full predicate.
#[inline(never)]
pub(in crate::exec) fn projected(plan: Plan, output: Option<Columns>, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::IndexJoin { outer, spatial, left, table, index, probe, cond, width, columns: needed, located, .. } = plan else {
        return failed(broken("an index join is not one"));
    };
    let inner = matches!(outer, Preserved::Inner);
    let opened = opened(db, &table, &index)
        .and_then(|(storage, found, ix, routes)| Ok((storage, found, ix, routes, Matched::of(outer, &needed, db)?)));
    match opened {
        Ok((storage, found, ix, routes, matched)) => {
            let joined = IndexJoin {
                left: Box::new(std::iter::empty()),
                table: Probed {
                    storage,
                    reading: Reading::of(&ix, Some(&needed), found.key_projection()),
                    index: found,
                    ix,
                    tx,
                    stop: db.tx.stop_of(tx),
                    located,
                    routes,
                },
                budget: prepared::budget(db).filter(|_| matches!(probe, JoinProbe::Predicate { .. })).map(Arc::clone),
                prepared: None,
                probe,
                cond,
                nulls: (!inner).then(|| vec![Value::Null; width]),
                matched,
                runner: Runner::new(db, tx),
                current: None,
                candidates: 0,
                output,
                refinement: None,
            };
            driven(joined, (spatial, *left), (inner, needed.into_vec()), db)
        }
        Err(e) => failed(e),
    }
}

/// What an index join of `table` by `index` reads: the table's rows, the
/// index, its key's shape, and \[EXTENSION\] of a partitioned table's
/// template, the routes of its probes (`indexed/routed.rs`).
fn opened(db: &Arc<State>, table: &QualifiedName, index: &str) -> Result<Opened> {
    if let Some((storage, found, ix, routes)) = routed::opened(db, table, index)? {
        return Ok((storage, found, ix, routes));
    }
    let storage = Arc::clone(db.storage_of(table)?);
    let ix = db.catalog.index(index).map(IndexDesc::key_shape).ok_or_else(|| broken(&format!("no index {index}")))?;
    Ok((storage, Arc::clone(db.index_of(index)?), ix, None))
}

/// `joined` driven by the rows of `left`, or, of a spatial join, perhaps
/// by a scan of its table (`indexed/adaptive.rs`) or a traversal of both
/// tables' trees (`indexed/paired.rs`).
fn driven(
    mut joined: IndexJoin,
    (spatial, left): (Option<Box<Spatial>>, Plan),
    (inner, needed): (bool, Vec<usize>),
    db: &Arc<State>,
) -> RowIter {
    match spatial.map(|spatial| *spatial) {
        Some(Spatial::Paired(paired)) => paired::start(joined, (*paired, left), db),
        Some(Spatial::Adaptive) => {
            joined.left = execute(left, db, joined.table.tx);
            if inner { adaptive::start(joined, needed, db) } else { Box::new(joined) }
        }
        None => {
            joined.left = execute(left, db, joined.table.tx);
            Box::new(joined)
        }
    }
}

/// The table an index join probes: its storage, the index, its columns,
/// what every fetch of its rows shares, read as `tx` sees them, the
/// statement's stop looked at, and whether each row is followed by its id
/// (`LogicalPlan::Scan`).
struct Probed {
    storage: Arc<dyn TableStorage>,
    index: Arc<dyn Index>,
    ix: KeyShape,
    /// The columns the query uses, chosen once, and the leaf last read, so
    /// rows side by side are fetched from it (`exec/entries.rs`).
    reading: Reading,
    tx: TxId,
    stop: Option<Arc<Stop>>,
    located: bool,
    /// \[EXTENSION\] Of a partitioned table's template, each partition's
    /// index, a probe routed to those its values may be in.
    routes: Option<Routes>,
}

impl Probed {
    /// Binds the outer row and records the comparison read, or for a
    /// spatial predicate the region the index reads, or else the whole
    /// table (`exec/read.rs`). A NULL operand selects no entries.
    fn entries(&mut self, binding: &JoinProbe, row: &Row) -> Result<Entries> {
        if let Some(stop) = &self.stop {
            stop.check()?;
        }
        let Some(probe) = binding.bind(row, self.ix.columns.len())? else {
            return Ok(Box::new(std::iter::empty()));
        };
        Asked::of(&*self.index, &self.ix, &probe).record(&*self.storage, self.tx)?;
        self.reading.next_probe();
        let entries = match &self.routes {
            Some(routes) => routes.search(&probe),
            None => Arc::clone(&self.index).search(&probe, KeyOrder::Ascending),
        };
        Ok(Box::new(checked(entries, self.stop.clone())))
    }

    /// The row of the entry `(key, id)`, if the version `tx` sees has it,
    /// followed by its id if located.
    fn row(&mut self, entry: Result<(Key, RowId)>) -> Result<Option<Row>> {
        let (key, id) = entry?;
        let found = fetch(&*self.storage, &self.ix, &mut self.reading, (self.tx, id), &key)?;
        Ok(found.map(|(_, mut row)| {
            if self.located {
                row.push(storage::located(id));
            }
            row
        }))
    }
}

/// [X3.135-1986 §5.20 GR2b, §5.21 GR2; ISO/IEC 9075:1992 §7.5] each left
/// row with the rows of the table its keys find that `cond` is true of,
/// or, if `nulls` (a LEFT join), with them if there are none.
struct IndexJoin {
    left: RowIter,
    table: Probed,
    /// The outer columns and comparisons bound for each search.
    probe: JoinProbe,
    cond: Option<Cond>,
    /// Optional scalar implementations prepared for the current outer row,
    /// under the same optimizer switch and memory budget as constant calls.
    budget: Option<Arc<MemoryBudget>>,
    prepared: Option<Cond>,
    /// Of a LEFT or FULL join, the nulls of a table row.
    nulls: Option<Vec<Value>>,
    /// Of a FULL join, the table rows paired, and then those that were not.
    matched: Option<Matched>,
    runner: Runner,
    /// The left row being joined, the entries found for it, being read,
    /// and whether one was kept.
    current: Option<(Row, Entries, bool)>,
    /// Candidate fetches of the current or just-completed probe. An
    /// adaptive join observes this work before requesting extra estimates.
    candidates: usize,
    output: Option<Columns>,
    /// A single spatial conjunct can refine borrowed rows before their
    /// selected output is built. Residuals retain the full row evaluator.
    refinement: Option<(Binding, ScalarRef)>,
}

enum Step {
    Row(Row),
    ProbeEnd,
    End,
}

impl IndexJoin {
    /// Probe boundaries let adaptive execution change the remaining work
    /// without repeating any outer row whose results have been emitted.
    fn advance(&mut self) -> Result<Step> {
        loop {
            if self.matched.as_ref().is_some_and(Matched::started) {
                return self.unmatched();
            }
            let Some((l, mut entries, found)) = self.current.take() else {
                let Some(l) = self.left.next().transpose()? else { return self.unmatched() };
                let entries = self.table.entries(&self.probe, &l)?;
                self.prepare(&l)?;
                self.current = Some((l, entries, false));
                self.candidates = 0;
                continue;
            };
            let Some(entry) = entries.next() else {
                self.prepared = None;
                self.refinement = None;
                // Its entries read: of a LEFT join, kept with nulls if none was.
                if let (Some(nulls), false) = (&self.nulls, found) {
                    let row = l.into_iter().chain(nulls.iter().cloned()).collect();
                    return self.project(row).map(Step::Row);
                }
                return Ok(Step::ProbeEnd);
            };
            self.candidates = self.candidates.saturating_add(1);
            let id = entry.as_ref().ok().map(|(_, id)| *id);
            let kept = self.paired(&l, entry);
            let paired = matches!(kept, Ok(Some(_)));
            self.current = Some((l, entries, found || paired));
            if let (true, Some(matched), Some(id)) = (paired, self.matched.as_mut(), id) {
                matched.add(id)?;
            }
            if let Some(joined) = kept? {
                return Ok(Step::Row(joined));
            }
        }
    }

    /// `l` joined with the row of `entry`, if the version read has it and
    /// the condition is true of the pair.
    fn paired(&mut self, l: &Row, entry: Result<(Key, RowId)>) -> Result<Option<Row>> {
        let Some(r) = self.table.row(entry)? else { return Ok(None) };
        if let (Some(output), Some((binding, function))) = (&self.output, &self.refinement) {
            let kept = binding.intersects(l, &r, function, self.runner.temp_lobs())?;
            return kept.then(|| output.pair(l, &r)).transpose();
        }
        let joined: Row = l.iter().cloned().chain(r).collect();
        let kept = match self.prepared.as_ref().or(self.cond.as_ref()) {
            Some(c) => eval_cond(c, &joined, &self.runner)? == Truth::True,
            None => true,
        };
        kept.then(|| self.project(joined)).transpose()
    }

    /// Of a FULL join whose outer rows are all probed, the next table row
    /// no probe paired; else the end.
    fn unmatched(&mut self) -> Result<Step> {
        let Some(matched) = self.matched.as_mut() else { return Ok(Step::End) };
        match matched.next(&self.table, &self.runner)? {
            Some(row) => self.project(row).map(Step::Row),
            None => Ok(Step::End),
        }
    }

    fn project(&self, row: Row) -> Result<Row> {
        match &self.output {
            Some(output) => output.take(row),
            None => Ok(row),
        }
    }

    fn prepare(&mut self, row: &Row) -> Result<()> {
        self.refinement = None;
        self.prepared = None;
        if self.output.is_some()
            && self.nulls.is_none()
            && let Some(binding) = Binding::of(self, row.len()).filter(|binding| binding.rest.is_none())
        {
            let function = binding.prepared(row, self.budget.as_ref())?;
            self.refinement = Some((binding, function));
            return Ok(());
        }
        self.prepared = self.cond.as_ref().zip(self.budget.as_ref()).map(|(cond, budget)| prepared::joining(cond, row, budget));
        Ok(())
    }
}

impl Iterator for IndexJoin {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            match self.advance() {
                Ok(Step::Row(row)) => return Some(Ok(row)),
                Ok(Step::End) => return None,
                Ok(Step::ProbeEnd) => {}
                Err(error) => return Some(Err(error)),
            }
        }
    }
}

// Unit tests: plans the executor cannot run (RULES.md R-TEST-1, R-ERR-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{MemoryCatalog, QualifiedName, TableDesc};
    use crate::engine::rows::{ROW_STORE, RowStore, TableOptions};
    use crate::error::Code;
    use crate::storage::StorageEngine;
    use crate::txn::NO_TRANSACTION;

    /// An index join of no left rows, of the table `A.E` by the index `I`.
    fn join_of_e() -> Plan {
        let name = QualifiedName { auth: "A".into(), ident: "E".into() };
        Plan::IndexJoin {
            outer: Preserved::Inner,
            spatial: None,
            left: Box::new(Plan::Values { rows: Vec::new(), detail: String::new() }),
            table: name,
            index: "I".into(),
            method: "".into(),
            probe: JoinProbe::Prefix(Box::new([])),
            cond: None,
            width: 0,
            columns: Box::new([]),
            detail: "".into(),
            est: 0.0_f64,
            cost: 0.0_f64,
            located: false,
        }
    }

    /// Verifies: [DESIGN] an index join given a plan of another kind, of a
    /// table the state lacks, or of an index it lacks, yields SQLCODE -910
    /// rather than panicking (RULES.md R-ERR-1).
    #[test]
    fn a_join_of_what_is_not_there_is_reported() {
        let code = |st: &Arc<State>, plan: Plan| indexed(plan, st, NO_TRANSACTION).collect::<Result<Vec<Row>>>().err().map(|e| e.code);
        let mut st = State::with_builtins(Box::new(MemoryCatalog::default()));
        let empty = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
        assert_eq!(code(&empty, Plan::Values { rows: Vec::new(), detail: String::new() }), Some(Code::Api));
        assert_eq!(code(&empty, join_of_e()), Some(Code::Api), "no table");
        let desc = TableDesc {
            periods: Vec::new(),
            typed: None,
            name: QualifiedName { auth: "A".into(), ident: "E".into() },
            columns: vec![],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        st.storage
            .insert(Arc::new(desc.name.clone()), Arc::from(RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap()));
        assert_eq!(code(&Arc::new(st), join_of_e()), Some(Code::Api), "no index");
    }
}
