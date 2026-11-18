//! \[EXTENSION\] An approximate index's nearest read (the vector design
//! document R9.2, R9.3, R9.5, R13.2; RULES.md D94). The index gives
//! candidates under its relaxed contract; each is fetched as the
//! transaction sees it, checked against the whole WHERE condition, and
//! measured by the exact SQL distance, its re-rank. The best of them, by
//! that distance and then by row (R9.5), go to the ordinary sort, which is
//! stable, so ties keep the row order. If fewer than the rows asked
//! qualify, the search is asked again twice as wide (R9.3), up to
//! `MOST_WIDENINGS` times; then the table is read exactly (R9.2), or, of
//! VECTOR.APPROX_FALLBACK OFF, the statement is refused. The whole-table
//! serializable read and cancellation are those of the certified read.

use std::cmp::Ordering;
use std::sync::Arc;

use super::super::entries::{Reading, fetch};
use super::super::work::Work;
use super::super::{RowIter, Runner, broken, eval_cond};
use super::{measured, scanned};
use crate::catalog::{IndexDesc, KeyShape};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key, NearestQuery, Probe, RowFilter};
use crate::logical::{Cond, Expr, Strategy, Truth, render_expr};
use crate::memory::Reservation;
use crate::physical::{ApproximateRead, NearestRead};
use crate::spill::row_bytes;
use crate::storage::{Condition, Row, RowId, TableStorage};
use crate::txn::{TxId, checked};
use crate::value::{CompOp, Value};

/// \[EXTENSION\] How many times a search that finds too few rows is asked
/// again, each twice as wide as the last (R9.3): up to 16 times the first.
pub const MOST_WIDENINGS: u32 = 4;
/// The name the rows kept reserve memory under (RULES.md D38).
const RESERVATION: &str = "approximate nearest rows";

/// The read of `read` through its approximate index, of `query`.
///
/// # Errors
///
/// Those of the index, the table and the condition; SQLCODE -900 if the
/// search finds too few rows, or the index declines, and
/// VECTOR.APPROX_FALLBACK is OFF.
pub(super) fn approximate(
    read: NearestRead,
    storage: Arc<dyn TableStorage>,
    db: &Arc<State>,
    (tx, query): (TxId, NearestQuery),
) -> Result<RowIter> {
    let Some(plan) = read.approximate.clone() else { return Err(broken("an approximate read asks nothing approximate")) };
    let index = Arc::clone(db.index_of(&read.index)?);
    let key = db.catalog.index(&read.index).map(IndexDesc::key_shape).ok_or_else(|| broken("a nearest index has no definition"))?;
    storage.read_by_condition(tx, &Condition { columns: Vec::new(), probe: Probe::All })?;
    let radius = radius(&read);
    let search = Searching { read: &read, storage: &*storage, key, db, tx, runner: Runner::new(db, tx) };
    let mut widen = 1_u32;
    for _ in 0..=MOST_WIDENINGS {
        let asked = NearestQuery { approximate: Some(plan.approximation(read.first, widen, radius)), ..query.clone() };
        let Some(kept) = search.search(&index, &asked, &plan)? else { break };
        if kept.qualified >= read.first {
            let rows = kept.best(read.first);
            return Ok(Box::new(checked(rows.into_iter().map(Ok), db.tx.stop_of(tx))));
        }
        widen = widen.saturating_mul(2);
    }
    if !plan.fallback {
        return Err(SqlError::new(
            Code::Unsupported,
            "[EXTENSION] the approximate search found fewer rows than asked, and VECTOR.APPROX_FALLBACK is OFF",
        ));
    }
    Ok(scanned(read, storage, db, tx, false))
}

/// \[EXTENSION\] Of a range search (§9.4, R9.1), the distance the
/// condition keeps rows below or at: a conjunct comparing the measure
/// with a number, either side.
fn radius(read: &NearestRead) -> Option<f64> {
    let measure = render_expr(&read.measure, &[]);
    let mut found = None;
    for conjunct in read.condition.iter().flat_map(conjuncts) {
        let Cond::Compare(a, op, b) = conjunct else { continue };
        let bound = match (op, a, b) {
            (CompOp::Lt | CompOp::Le, e, Expr::Const(v)) | (CompOp::Gt | CompOp::Ge, Expr::Const(v), e) => {
                (render_expr(e, &[]) == measure).then(|| v.as_f64()).flatten()
            }
            (_, _, _) => None,
        };
        if let Some(r) = bound {
            found = Some(found.map_or(r, |f: f64| f.min(r)));
        }
    }
    found
}

/// The conjuncts of `c`.
fn conjuncts(c: &Cond) -> Vec<&Cond> {
    if let Cond::And(a, b) = c {
        let mut all = conjuncts(a);
        all.extend(conjuncts(b));
        return all;
    }
    vec![c]
}

/// What a search reads and evaluates with.
struct Searching<'r> {
    read: &'r NearestRead,
    storage: &'r dyn TableStorage,
    key: KeyShape,
    db: &'r Arc<State>,
    tx: TxId,
    runner: Runner,
}

impl Searching<'_> {
    /// The rows the index's candidates for `query` give, or none if it
    /// declines, or the rows kept cannot be afforded.
    fn search(&self, index: &Arc<dyn Index>, query: &NearestQuery, plan: &ApproximateRead) -> Result<Option<Kept>> {
        let mut reading = Reading::of(&self.key, Some(&self.read.columns), index.key_projection());
        let entries = if plan.strategy == Strategy::In && self.read.condition.is_some() {
            let mut filter = Filter { search: self, reading: Reading::of(&self.key, Some(&self.read.columns), index.key_projection()) };
            Arc::clone(index).approximate(query, &self.db.memory, Some(&mut filter))?
        } else {
            Arc::clone(index).approximate(query, &self.db.memory, None)?
        };
        let Some(entries) = entries else { return Ok(None) };
        let Some(mut kept) = Kept::new(self.db) else { return Ok(None) };
        for entry in checked(entries, self.db.tx.stop_of(self.tx)) {
            let (_, key, id) = entry?;
            let Some((id, row)) = fetch(self.storage, &self.key, &mut reading, (self.tx, id), &key)? else { continue };
            let Some(row) = measured(self.read, (id, row), &self.runner)? else { continue };
            match row.last() {
                Some(Value::Null) => {}
                Some(Value::Approx(distance)) => {
                    if !kept.add((*distance, id, row), self.read.first) {
                        return Ok(None);
                    }
                }
                Some(_) | None => return Err(broken("a nearest distance is neither DOUBLE PRECISION nor NULL")),
            }
        }
        Ok(Some(kept))
    }
}

/// [R13.2] The in-filter: whether a candidate's visible row meets the
/// WHERE condition, read and evaluated as the read itself does.
struct Filter<'s, 'r> {
    search: &'s Searching<'r>,
    reading: Reading,
}

impl RowFilter for Filter<'_, '_> {
    fn admits(&mut self, key: &Key, row: RowId) -> Result<bool> {
        let s = self.search;
        let Some((id, mut row)) = fetch(s.storage, &s.key, &mut self.reading, (s.tx, row), key)? else { return Ok(false) };
        let Some(condition) = &s.read.condition else { return Ok(true) };
        if s.read.located {
            row.push(crate::storage::located(id));
        }
        Ok(eval_cond(condition, &row, &s.runner)? == Truth::True)
    }
}

/// The best rows a search found, by distance and then row (R9.5), at
/// most twice the rows asked held at once, within MEMORY.WORK_BYTES and
/// the database's budget; and how many qualified.
struct Kept {
    rows: Vec<(f64, RowId, Row)>,
    qualified: usize,
    bytes: usize,
    limit: usize,
    held: Reservation,
}

/// The order rows are kept in: distance, then row (R9.5).
fn order(a: &(f64, RowId, Row), b: &(f64, RowId, Row)) -> Ordering {
    a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))
}

impl Kept {
    fn new(db: &State) -> Option<Kept> {
        let held = db.memory.reserve(0, RESERVATION).ok()?;
        Some(Kept { rows: Vec::new(), qualified: 0, bytes: 0, limit: Work::new(db).limit(), held })
    }

    /// Takes a qualifying row; false if it cannot be afforded.
    fn add(&mut self, row: (f64, RowId, Row), first: usize) -> bool {
        self.qualified = self.qualified.saturating_add(1);
        self.bytes = self.bytes.saturating_add(row_bytes(&row.2));
        self.rows.push(row);
        if self.rows.len() > first.saturating_mul(2) {
            self.rows.sort_by(order);
            self.rows.truncate(first);
            self.bytes = self.rows.iter().map(|r| row_bytes(&r.2)).sum();
        }
        self.bytes <= self.limit && self.held.resize(self.bytes).is_ok()
    }

    /// Its best `first` rows, in order.
    fn best(mut self, first: usize) -> Vec<Row> {
        self.rows.sort_by(order);
        self.rows.truncate(first);
        self.rows.into_iter().map(|(_, _, row)| row).collect()
    }
}
