//! The index nested-loop join (`physical/indexed.rs`, RULES.md D35): the
//! rows of a product its equalities keep [X3.135-1986 §5.20 GR2b, §5.21
//! GR2], or of a LEFT outer join [ISO/IEC 9075:1992 §7.5], each row of
//! the left operand joined with the rows an index of the right's table
//! finds for its values, as they are found, those the rest of the
//! condition is true of kept; of a LEFT join, a left row none is kept of
//! with nulls. The left is read as it is pulled and nothing is held: its
//! rows' order is the result's, each row's matches in the index's. The
//! statement looks for a cancel before each probe and as each entry is
//! read (RULES.md D44). Split from exec/join.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::super::{RowIter, Runner, broken, column, eval_cond, execute, failed, fetch};
use crate::db::State;
use crate::error::Result;
use crate::index::{Entries, Index, Key, KeyOrder, Probe};
use crate::logical::{Cond, Truth};
use crate::physical::Plan;
use crate::storage::{Condition, Row, RowId, TableStorage};
use crate::txn::{Stop, TxId, checked};
use crate::value::{CompOp, Value};

/// The rows of the index nested-loop join `plan`. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack (RULES.md
/// R-CF-6).
#[inline(never)]
pub(in crate::exec) fn indexed(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::IndexJoin { outer, left, table, index, keys, cond, width, .. } = plan else {
        return failed(broken("an index join is not one"));
    };
    let opened = db.storage_of(&table).map(Arc::clone).and_then(|storage| {
        let columns = db.catalog.index(&index).map(|ix| ix.columns.clone()).ok_or_else(|| broken(&format!("no index {index}")))?;
        Ok((storage, Arc::clone(db.index_of(&index)?), columns))
    });
    match opened {
        Ok((storage, found, columns)) => Box::new(IndexJoin {
            left: execute(*left, db, tx),
            table: Probed { storage, index: found, columns, tx, stop: db.tx.stop_of(tx) },
            keys,
            cond,
            nulls: outer.then(|| vec![Value::Null; width]),
            runner: Runner::new(db, tx),
            current: None,
        }),
        Err(e) => failed(e),
    }
}

/// The table an index join probes: its storage, the index, its columns,
/// read as `tx` sees them, the statement's stop looked at.
struct Probed {
    storage: Arc<dyn TableStorage>,
    index: Arc<dyn Index>,
    columns: Vec<usize>,
    tx: TxId,
    stop: Option<Arc<Stop>>,
}

impl Probed {
    /// The entries of the index whose first columns' keys are `values`:
    /// none if one is null, which equals nothing [X3.135-1986 §5.11 GR2].
    /// The engine is told which rows are read by them
    /// (`TableStorage::read_by_condition`).
    fn entries(&self, values: Vec<Value>) -> Result<Entries> {
        if let Some(stop) = &self.stop {
            stop.check()?;
        }
        if values.iter().any(Value::is_null) {
            return Ok(Box::new(std::iter::empty()));
        }
        // An index of one column is asked by a comparison, which every
        // method answers that supports equality.
        let probe = match (values.len(), self.columns.len()) {
            (1, 1) => Probe::Compare { op: CompOp::Eq, value: values.into_iter().next().unwrap_or(Value::Null) },
            _ => Probe::Prefix { equal: values, then: None },
        };
        self.storage.read_by_condition(self.tx, &Condition { columns: self.columns.clone(), probe: probe.clone() })?;
        let entries = Arc::clone(&self.index).search(&probe, KeyOrder::Ascending);
        Ok(Box::new(checked(entries, self.stop.clone())))
    }

    /// The row of the entry `(key, id)`, if the version `tx` sees has it.
    fn row(&self, entry: Result<(Key, RowId)>) -> Result<Option<Row>> {
        let (key, id) = entry?;
        Ok(fetch(&*self.storage, &self.columns, self.tx, &key, id)?.map(|(_, row)| row))
    }
}

/// [X3.135-1986 §5.20 GR2b, §5.21 GR2; ISO/IEC 9075:1992 §7.5] each left
/// row with the rows of the table its keys find that `cond` is true of,
/// or, if `nulls` (a LEFT join), with them if there are none.
struct IndexJoin {
    left: RowIter,
    table: Probed,
    /// The left columns whose values are probed for, of the index's first
    /// columns in order.
    keys: Vec<usize>,
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
                let values = self.keys.iter().map(|&k| column(&l, k).cloned()).collect::<Result<Vec<_>>>()?;
                self.current = Some((l, self.table.entries(values)?, false));
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
    fn paired(&self, l: &Row, entry: Result<(Key, RowId)>) -> Result<Option<Row>> {
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
            keys: Vec::new(),
            cond: None,
            width: 0,
            detail: String::new(),
            est: 0.0_f64,
            cost: 0.0_f64,
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
            name: QualifiedName { auth: "A".into(), ident: "E".into() },
            columns: vec![],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        st.storage.insert(desc.name.clone(), Arc::from(RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap()));
        assert_eq!(code(&Arc::new(st), join_of_e()), Some(Code::Api), "no index");
    }
}
