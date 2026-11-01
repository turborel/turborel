//! \[DESIGN\] Folding an assertion's kept totals (`kept.rs`, RULES.md D51,
//! D55): a table of them gains a row of each form at each statement that
//! changes it, so a read that finds more than `FOLD_ROWS` notes the table
//! (`Transactions::note_fold`), and the session ending a transaction then
//! asks for the database's job `turborel-assertion-totals`, which the
//! thread manager runs as JOBS.POLICY says.
//!
//! The job folds each table noted in a transaction of its own: of the rows
//! its snapshot holds, one of each form holding what they held together,
//! the others deleted. Every snapshot reads the same totals before and
//! after, so its writes are marked as keeping them
//! (`TableStorage::preserving`): no other transaction's commit is refused
//! for reading what it rewrites, and it is checked against none. Rows
//! written meanwhile, which its snapshot does not hold, it leaves. A
//! failure, as of a table dropped meanwhile, leaves the rows for a later
//! fold, and is told (`Event::Thread`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use super::super::{Database, State};
use super::kept::{Move, Totals, WIDTH, kept_row, parse_row};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::RowId;

/// \[DESIGN\] How many rows a table of kept totals holds, seen by a read,
/// before it is noted to be folded: enough that a fold is rare against
/// the statements that wrote them, few enough that a read stays short.
pub(super) const FOLD_ROWS: usize = 64;

/// The folding job's work: each table of kept totals noted, folded.
///
/// # Errors
///
/// SQLCODE -910 if the database is closed; the first fold's error, the
/// others' left for later.
pub(in crate::db) fn folded(db: &Database) -> Result<Option<Duration>> {
    let st = db.snapshot()?;
    let mut first = None;
    for table in st.tx.take_folds() {
        if let Err(e) = st.fold_totals(&table) {
            first.get_or_insert(e);
        }
    }
    first.map_or(Ok(None), Err)
}

impl State {
    /// Folds the rows of `table`, of kept totals, its snapshot holds: one of
    /// each form, the others deleted, in a transaction whose writes keep
    /// every total (`TableStorage::preserving`).
    ///
    /// # Errors
    ///
    /// The table's reads' or writes', or a commit's; the transaction is
    /// rolled back.
    pub(super) fn fold_totals(self: &Arc<Self>, table: &QualifiedName) -> Result<()> {
        if self.catalog.table(table).is_none() {
            return Ok(());
        }
        let tx = self.begin();
        let folded = self.fold_in(&tx, table);
        match folded {
            Ok(true) => self.commit(&tx),
            Ok(false) => self.rollback(&tx),
            Err(e) => self.rollback(&tx).and(Err(e)),
        }
    }

    /// `fold_totals`'s work, in transaction `tx`: whether it wrote any.
    fn fold_in(self: &Arc<Self>, tx: &crate::txn::Txn, table: &QualifiedName) -> Result<bool> {
        self.read_lock(tx, table)?;
        let storage = Arc::clone(self.storage_of(table)?);
        storage.preserving(tx.id)?;
        let mut forms: BTreeMap<usize, (Vec<RowId>, Totals)> = BTreeMap::new();
        for found in Arc::clone(&storage).scan(tx.id, &(0..WIDTH).collect::<Vec<_>>()) {
            let (id, row) = found?;
            let (form, totals) = parse_row(&row)?;
            let (ids, sum) = forms.entry(form).or_default();
            ids.push(id);
            *sum = sum.plus(totals)?;
        }
        if forms.values().all(|(ids, _)| ids.len() < 2) {
            return Ok(false);
        }
        for (form, (ids, sum)) in forms {
            for id in ids {
                self.delete_row(tx, table, id)?;
            }
            if !sum.is_zero() {
                self.insert_row(tx, table, &kept_row(form, &sum, Move::Still))?;
            }
        }
        Ok(true)
    }
}
