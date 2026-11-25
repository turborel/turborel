//! \[DESIGN\] (RULES.md D1150) What a thread holds of a morsel as it reads
//! it, charged to the part's work grant as it grows (RULES.md D38, D298).
//! A morsel's rows are as many as its joins' matches make them, however
//! few its table gives, so what a thread gathers of one is charged a batch
//! at a time (`Tally`), and a morsel that outgrows the grant is given up,
//! what it was charged given back (`Taken::Past`). What reads the part
//! then does what it does of rows past its grant: a stream reads that
//! morsel itself, a row or a batch at a time, holding none (`Flow`); a
//! sort writes it to its runs; a window's or a set operation's partitions
//! put it in their lists, which spill; a join's build gives way to the
//! plan run alone.
//!
//! A morsel's rows gathered are charged until whoever reads them has
//! taken them, so every morsel gathered and not yet read, as many as run
//! ahead of the reader (`parallel/run.rs`), is held within the grant.
//! Before, each was held whole outside it, and a join whose keys each
//! matched thousands of rows made tens of millions of rows of a morsel.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::Limit;
use super::run::Morsels;
use crate::error::Result;
use crate::exec::batch::{Batches, Kept};
use crate::exec::{RowIter, broken};
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::txn::Stop;

/// What a thread made of a morsel: all of it, with the bytes it holds of
/// the grant, or nothing, the morsel past the grant.
pub(super) enum Taken<T> {
    Whole(T, usize),
    Past,
}

/// The bytes one morsel holds of its part's grant, as the thread reading
/// it counts them.
pub(super) struct Tally {
    limit: Limit,
    taken: AtomicUsize,
    past: AtomicBool,
}

impl Tally {
    /// A morsel's bytes, none yet, charged to `limit`.
    pub(super) fn of(limit: &Limit) -> Arc<Tally> {
        Arc::new(Tally { limit: limit.clone(), taken: AtomicUsize::new(0), past: AtomicBool::new(false) })
    }

    /// Charges `bytes` more: whether the grant holds them. Once it does
    /// not, the morsel is past it.
    pub(super) fn add(&self, bytes: usize) -> bool {
        if self.limit.take(bytes) {
            self.taken.fetch_add(bytes, Ordering::AcqRel);
            return true;
        }
        self.limit.give(bytes);
        self.past.store(true, Ordering::Release);
        false
    }

    /// Whether the morsel outgrew the grant.
    pub(super) fn past(&self) -> bool {
        self.past.load(Ordering::Acquire)
    }

    /// Charges the morsel `bytes` in all, what it holds once made, in
    /// place of what it was charged while read: whether the grant holds
    /// them.
    pub(super) fn settle(&self, bytes: usize) -> bool {
        let taken = self.taken.swap(0, Ordering::AcqRel);
        if bytes <= taken {
            self.limit.give(taken.saturating_sub(bytes));
            self.taken.store(bytes, Ordering::Release);
            return !self.past();
        }
        self.taken.store(taken, Ordering::Release);
        self.add(bytes.saturating_sub(taken))
    }

    /// `made` of the morsel, with the bytes it is charged; or, of a
    /// morsel past the grant, nothing, its bytes given back.
    pub(super) fn taken<T>(&self, made: T) -> Taken<T> {
        let taken = self.taken.swap(0, Ordering::AcqRel);
        if self.past() {
            self.limit.give(taken);
            return Taken::Past;
        }
        Taken::Whole(made, taken)
    }
}

/// \[DESIGN\] The rows charged to a grant at once: a batch's worth, as a
/// morsel read by batch is charged a batch at a time, so a thread looks
/// at the grant shared by every thread once a batch, not once a row.
const CHARGED_ROWS: usize = crate::storage::BATCH_ROWS;

/// Items of `inner` charged to a tally, every `every` of them at once,
/// their bytes as `bytes` counts them, and what is left once they end;
/// ended at the first charge the grant does not hold, which the tally then
/// says (`Tally::past`).
struct Charged<T> {
    inner: Box<dyn Iterator<Item = Result<T>> + Send>,
    tally: Arc<Tally>,
    bytes: fn(&T) -> usize,
    every: usize,
    /// The items, and their bytes, given since the last charge.
    since: (usize, usize),
    ended: bool,
}

impl<T> Charged<T> {
    /// Charges what was given since the last charge: whether it fits.
    fn charged(&mut self) -> bool {
        let (_, bytes) = std::mem::take(&mut self.since);
        bytes == 0 || self.tally.add(bytes)
    }
}

impl<T> Iterator for Charged<T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        if self.ended {
            return None;
        }
        let Some(item) = self.inner.next() else {
            self.ended = true;
            self.charged();
            return None;
        };
        if let Ok(given) = &item {
            self.since = (self.since.0.saturating_add(1), self.since.1.saturating_add((self.bytes)(given)));
            if self.since.0 >= self.every && !self.charged() {
                self.ended = true;
                return None;
            }
        }
        Some(item)
    }
}

/// `batches`, each charged to `tally` as it is given, the bytes of its
/// vectors; ended at the first the grant does not hold, which `tally`
/// then says (`Tally::past`).
pub(super) fn charged_batches(batches: Batches, tally: &Arc<Tally>) -> Batches {
    let bytes: fn(&Kept) -> usize = |(batch, _)| batch.bytes();
    Box::new(Charged { inner: batches, tally: Arc::clone(tally), bytes, every: 1, since: (0, 0), ended: false })
}

/// `rows`, charged to `tally` as they are given, `CHARGED_ROWS` at a time;
/// ended at the first charge the grant does not hold, which `tally` then
/// says (`Tally::past`).
pub(super) fn charged_rows(rows: RowIter, tally: &Arc<Tally>) -> RowIter {
    Box::new(Charged { inner: rows, tally: Arc::clone(tally), bytes: row_bytes, every: CHARGED_ROWS, since: (0, 0), ended: false })
}

/// The rows of `rows`, a morsel's, gathered within `limit`.
///
/// # Errors
///
/// As reading a row fails.
pub(super) fn whole_rows(rows: RowIter, limit: &Limit) -> Result<Taken<Vec<Row>>> {
    let tally = Tally::of(limit);
    let rows = charged_rows(rows, &tally).collect::<Result<Vec<Row>>>()?;
    Ok(tally.taken(rows))
}

/// The batches of `batches`, a morsel's, gathered within `limit`.
///
/// # Errors
///
/// As reading a batch fails.
pub(super) fn whole_batches(batches: Batches, limit: &Limit) -> Result<Taken<Vec<Kept>>> {
    let tally = Tally::of(limit);
    let batches = charged_batches(batches, &tally).collect::<Result<Vec<Kept>>>()?;
    Ok(tally.taken(batches))
}

/// Reads morsel `n` of a pipeline alone, holding none of it.
pub(super) type Alone<T> = Box<dyn Fn(usize) -> Result<Box<dyn Iterator<Item = Result<T>> + Send>> + Send>;

/// What a flow gives now: a morsel gathered, or one read as it is given.
enum Now<T> {
    Whole(std::vec::IntoIter<T>),
    Alone(Box<dyn Iterator<Item = Result<T>> + Send>),
}

/// \[DESIGN\] (RULES.md D1150) A pipeline's rows, or batches, given a
/// morsel's at a time, in order: a morsel a thread gathered within the
/// grant, its bytes given back once it is read; one past the grant read
/// here as it is given, a row or a batch at a time, the threads gathering
/// those after it meanwhile. The statement's cancellation is looked at as
/// each morsel is taken up (RULES.md D44).
pub(super) struct Flow<T: Send + 'static> {
    morsels: Morsels<Taken<Vec<T>>>,
    alone: Alone<T>,
    now: Now<T>,
    /// The bytes the morsel given now holds of the grant.
    held: usize,
    /// The number of the next morsel.
    next: usize,
    limit: Limit,
    stop: Option<Arc<Stop>>,
    ended: bool,
}

impl<T: Send + 'static> Flow<T> {
    /// The flow of `morsels`, each gathered within `limit` or left to
    /// `alone`, for a statement `stop` cancels.
    pub(super) fn of(morsels: Morsels<Taken<Vec<T>>>, alone: Alone<T>, limit: &Limit, stop: Option<Arc<Stop>>) -> Flow<T> {
        Flow { morsels, alone, now: Now::Whole(Vec::new().into_iter()), held: 0, next: 0, limit: limit.clone(), stop, ended: false }
    }

    /// Takes up the next morsel: whether there is one.
    fn taken_up(&mut self) -> Result<bool> {
        self.limit.give(std::mem::take(&mut self.held));
        if let Some(stop) = &self.stop {
            stop.check()?;
        }
        let Some(taken) = self.morsels.next().transpose()? else { return Ok(false) };
        let n = self.next;
        self.next = n.checked_add(1).ok_or_else(|| broken("a pipeline of too many morsels"))?;
        self.now = match taken {
            Taken::Whole(items, bytes) => {
                self.held = bytes;
                Now::Whole(items.into_iter())
            }
            Taken::Past => Now::Alone((self.alone)(n)?),
        };
        Ok(true)
    }
}

impl<T: Send + 'static> Iterator for Flow<T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        loop {
            if self.ended {
                return None;
            }
            let item = match &mut self.now {
                Now::Whole(items) => items.next().map(Ok),
                Now::Alone(items) => items.next(),
            };
            match item {
                Some(Ok(item)) => return Some(Ok(item)),
                Some(Err(e)) => {
                    // A failed morsel ends its run: a cursor cut short stays failed.
                    self.ended = true;
                    return Some(Err(e));
                }
                None => {}
            }
            match self.taken_up() {
                Ok(true) => {}
                Ok(false) => return None,
                Err(e) => {
                    self.ended = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

// Unit tests: a morsel gathered within its grant (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Taken, whole_batches, whole_rows};
    use crate::catalog::QualifiedName;
    use crate::db::Database;
    use crate::exec::parallel::pipeline::Pipeline;
    use crate::exec::parallel::{Held, Limit};
    use crate::exec::{Db, Runner};
    use crate::physical::{JoinKey, Plan};
    use crate::txn::NO_TRANSACTION;
    use crate::value::{Decimal, Value};

    /// The scan of column K of table `t` of schema A.
    fn scan(t: &str) -> Box<Plan> {
        Box::new(Plan::SeqScan {
            table: QualifiedName { auth: "A".into(), ident: t.into() },
            columns: vec![0],
            rows: 0,
            cost: 0.0_f64,
            order: Vec::new(),
            backward: false,
            most: None,
            located: false,
        })
    }

    /// Of `db` as it is set, the first morsel of T joined with U on K
    /// gathered within a grant of MEMORY.WORK_BYTES, by batch if it is
    /// read so, else row by row: its rows, if it fit; and whether the
    /// grant holds what the build held once the morsel is given up.
    fn first_morsel(database: &Database) -> (Option<usize>, bool) {
        let join = Plan::HashJoin {
            left: scan("T"),
            right: scan("U"),
            keys: vec![JoinKey { left: 0, right: 0, nulls: false }],
            detail: String::new(),
            est: 0.0_f64,
        };
        let state = Db::from(database.snapshot().unwrap());
        let limit = Limit::of(&state);
        let Held::Kept(pipeline) = Pipeline::of(&join, (&state, NO_TRANSACTION), &limit).unwrap() else { panic!("the build gave way") };
        let (pipeline, built) = (Arc::new(pipeline), limit.held());
        let (db, tx) = (&state, NO_TRANSACTION);
        let rows = if pipeline.batched.is_some() {
            match whole_batches(pipeline.morsel_kept(0, (db, tx)).unwrap(), &limit).unwrap() {
                Taken::Whole(batches, _) => Some(batches.iter().map(|(b, _)| b.rows).sum()),
                Taken::Past => None,
            }
        } else {
            match whole_rows(pipeline.morsel_rows(0, (db, tx), Runner::new(db, tx)).unwrap(), &limit).unwrap() {
                Taken::Whole(rows, _) => Some(rows.len()),
                Taken::Past => None,
            }
        };
        (rows, rows.is_some() || limit.held() == built)
    }

    /// Verifies: [DESIGN] RULES.md D1150, D38: a morsel whose join makes
    /// a hundred rows of each of its own is gathered whole if its rows fit
    /// the part's grant, and is given up once they outgrow it, what it was
    /// charged given back, by batch and row by row; before, it was held
    /// whole whatever the grant.
    ///
    /// Decision: the morsel's rows fit the grant, or not; it is read by
    /// batch, or row by row.
    #[test]
    fn a_morsel_past_its_grant_is_given_up() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE TABLE U (K INTEGER)").unwrap();
        let a = db.session("A");
        let keys: Vec<String> = (0_i32..300_i32).map(|k| format!("({})", k % 3_i32)).collect();
        a.run(&format!("INSERT INTO T VALUES {}", keys.join(", "))).unwrap();
        a.run(&format!("INSERT INTO U VALUES {}", keys.join(", "))).unwrap();
        a.commit().unwrap();
        let set = |name: &str, value: Value| db.set_setting(name, &value).unwrap();
        set("EXECUTOR.MORSEL_ROWS", Value::Exact(Decimal::new(100_000, 0)));
        for batches in ["ON", "OFF"] {
            set("EXECUTOR.BATCHES", Value::Char(batches.into()));
            set("MEMORY.WORK_BYTES", Value::Exact(Decimal::new(64 << 20, 0)));
            assert_eq!(first_morsel(&db), (Some(30_000), true), "within the grant, batches {batches}");
            set("MEMORY.WORK_BYTES", Value::Exact(Decimal::new(65_536, 0)));
            assert_eq!(first_morsel(&db), (None, true), "past the grant, batches {batches}");
        }
    }
}
