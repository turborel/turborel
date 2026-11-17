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

use std::sync::Arc;

use super::super::entries::{Reading, fetch};
use super::super::{RowIter, Runner, broken, eval_cond, execute, failed};
use crate::catalog::{IndexDesc, KeyShape};
use crate::db::State;
use crate::error::Result;
use crate::index::{Entries, Index, Key, KeyOrder};
use crate::logical::{Cond, Truth};
use crate::physical::{JoinProbe, Plan};
use crate::storage::{self, Condition, Row, RowId, TableStorage};
use crate::txn::{Stop, TxId, checked};
use crate::value::Value;

/// The rows of the index nested-loop join `plan`. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack (RULES.md
/// R-CF-6).
#[inline(never)]
pub(in crate::exec) fn indexed(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::IndexJoin { outer, left, table, index, probe, cond, width, columns: needed, located, .. } = plan else {
        return failed(broken("an index join is not one"));
    };
    let opened = db.storage_of(&table).map(Arc::clone).and_then(|storage| {
        let ix = db.catalog.index(&index).map(IndexDesc::key_shape).ok_or_else(|| broken(&format!("no index {index}")))?;
        Ok((storage, Arc::clone(db.index_of(&index)?), ix))
    });
    match opened {
        Ok((storage, found, ix)) => Box::new(IndexJoin {
            left: execute(*left, db, tx),
            table: Probed { storage, index: found, reading: Reading::of(&ix, Some(&needed)), ix, tx, stop: db.tx.stop_of(tx), located },
            probe,
            cond,
            nulls: outer.then(|| vec![Value::Null; width]),
            runner: Runner::new(db, tx),
            current: None,
        }),
        Err(e) => failed(e),
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
}

impl Probed {
    /// Binds the outer row and records the exact predicate read. A NULL
    /// compared value selects no entries for either kind of probe.
    fn entries(&self, binding: &JoinProbe, row: &Row) -> Result<Entries> {
        if let Some(stop) = &self.stop {
            stop.check()?;
        }
        let Some(probe) = binding.bind(row, self.ix.columns.len())? else {
            return Ok(Box::new(std::iter::empty()));
        };
        self.storage.read_by_condition(self.tx, &Condition { columns: self.ix.columns.clone(), probe: probe.clone() })?;
        let entries = Arc::clone(&self.index).search(&probe, KeyOrder::Ascending);
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
    /// Of a LEFT join, the nulls of a table row.
    nulls: Option<Vec<Value>>,
    runner: Runner,
    /// The left row being joined, the entries found for it, being read,
    /// and whether one was kept.
    current: Option<(Row, Entries, bool)>,
}

impl IndexJoin {
    /// The next joined row, if there is one.
    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            let Some((l, mut entries, found)) = self.current.take() else {
                let Some(l) = self.left.next().transpose()? else { return Ok(None) };
                let entries = self.table.entries(&self.probe, &l)?;
                self.current = Some((l, entries, false));
                continue;
            };
            let Some(entry) = entries.next() else {
                // Its entries read: of a LEFT join, kept with nulls if none was.
                if let (Some(nulls), false) = (&self.nulls, found) {
                    return Ok(Some(l.into_iter().chain(nulls.iter().cloned()).collect()));
                }
                continue;
            };
            let kept = self.paired(&l, entry);
            self.current = Some((l, entries, found || matches!(kept, Ok(Some(_)))));
            if let Some(joined) = kept? {
                return Ok(Some(joined));
            }
        }
    }

    /// `l` joined with the row of `entry`, if the version read has it and
    /// the condition is true of the pair.
    fn paired(&mut self, l: &Row, entry: Result<(Key, RowId)>) -> Result<Option<Row>> {
        let Some(r) = self.table.row(entry)? else { return Ok(None) };
        let joined: Row = l.iter().chain(&r).cloned().collect();
        let kept = match &self.cond {
            Some(c) => eval_cond(c, &joined, &self.runner)? == Truth::True,
            None => true,
        };
        Ok(kept.then_some(joined))
    }
}

impl Iterator for IndexJoin {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
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
            outer: false,
            left: Box::new(Plan::Values { rows: Vec::new(), detail: String::new() }),
            table: name,
            index: "I".into(),
            method: String::new(),
            probe: JoinProbe::Prefix(Box::new([])),
            cond: None,
            width: 0,
            columns: Vec::new(),
            detail: String::new(),
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
