//! \[DESIGN\] (RULES.md D1080) A hash join's built operand put together
//! by the threads that read it. An operand of more rows than a morsel
//! holds, read by its morsels, was joined into one batch, hashed, placed
//! in its table and summed up in its runtime filters by the statement's
//! thread alone, while the workers that read it waited: the one stretch
//! of a join run in parallel that a single thread ran. Here each of those
//! is a run of morsels (`run.rs`), taken by the workers and the
//! statement's thread at once:
//!
//! - the batch, a vector of it to a thread, each joined of the morsels'
//!   vectors at its place (`Batch::joined_at`);
//! - the keys' hashes, or their numbers where they may be dense in their
//!   span (RULES.md D601), a run of `EXECUTOR.MORSEL_ROWS` rows to a
//!   thread, as a morsel is;
//! - those rows sorted by the table's parts, a run to a thread
//!   (`table/join/parted.rs`), once every run's keys said whether the
//!   table is direct;
//! - the table's parts, and the runtime filter of each key, one to a
//!   thread.
//!
//! No thread writes what another reads: each gives what it made back to
//! the run, in order, and the next run reads it. The table finds the rows
//! one thread's would, in the same order, and the filters are those the
//! table built alone makes (`batch/joined/shared.rs`). An operand of no
//! more rows than a morsel, or a part of one thread, is built by the
//! statement's thread as before, as its reading took one thread's work.

use std::sync::Arc;

use super::run::{Morsels, Work};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Filters, Frame, SharedTable, dense_wanted, wanted};
use crate::exec::table::{BUILD_PARTS, BuiltPart, JoinTable, KeyHashes, Parted, Parting, RuntimeFilter, Span, numbers_hashed};
use crate::physical::JoinKey;
use crate::settings::EXECUTOR_MORSEL_ROWS;
use crate::storage::{Batch, Column};
use crate::txn::TxId;

/// The threads a table is built on: up to `degree`, for the statement of
/// `tx`.
#[derive(Clone, Copy)]
pub(super) struct Crew<'d> {
    pub(super) db: &'d Db,
    pub(super) tx: TxId,
    pub(super) degree: usize,
}

/// A run of an operand's rows hashed: its keys' hashes, or their numbers,
/// and what the numbers say of a direct table, if they were asked.
type Hashed = (KeyHashes, Option<Span>);

/// What a thread makes of a table as its last run: a part of it, or a
/// key's runtime filter, if the key has one.
enum Made {
    Part(BuiltPart),
    Filter(Option<Arc<RuntimeFilter>>),
}

impl Crew<'_> {
    /// What `work` gives of each of `count` numbers, in order, taken by
    /// its threads at once.
    fn run<T: Send + 'static>(&self, count: usize, work: Work<T>) -> Result<Vec<T>> {
        Morsels::start(self.db, self.tx, (count, self.degree), work).collect()
    }

    /// The rows of a run of the hashing and the placing: a morsel's.
    fn run_rows(&self) -> usize {
        self.db.settings.count(EXECUTOR_MORSEL_ROWS).max(1)
    }
}

/// The table of the operand's rows `parts`, batches of its columns
/// `columns` of its rows `width` wide, every row kept, in order, joined on
/// `keys`: made by `crew`'s threads if it holds more rows than a morsel
/// and the crew more than one thread, else by the statement's.
///
/// # Errors
///
/// As `SharedTable::of`; a statement cancelled.
pub(super) fn shared(parts: Vec<Batch>, (columns, width): (Vec<usize>, usize), keys: &[JoinKey], crew: Crew<'_>) -> Result<SharedTable> {
    let rows = parts.iter().fold(0_usize, |n, part| n.saturating_add(part.rows));
    let seed = crew.db.functions.seed().clone();
    if crew.degree < 2 || rows <= crew.run_rows() {
        return SharedTable::of(parts, (columns, width), keys, (seed, dense_wanted(crew.db)));
    }
    let frame = Arc::new(Frame::of(joined(parts, crew)?, (columns, width), keys, seed)?);
    let numbers = dense_wanted(crew.db) && u32::try_from(rows).is_ok() && one_of_integers(&frame);
    let hashed = Arc::new(hashed(&frame, numbers, crew)?);
    let keyed = hashed.iter().fold(0_usize, |n, (h, _)| n.saturating_add(h.misses.iter().filter(|&&m| !m).count()));
    let span = hashed.iter().filter_map(|(_, span)| *span).reduce(Span::with);
    let parting = Parting::of(span, keyed);
    let runs = Arc::new(parted(&frame, &hashed, (parting, numbers), crew)?);
    drop(hashed);
    let (table, filters) = made(&frame, &runs, parting, (keys.len(), crew))?;
    // A worker may still be letting go of a run that holds the frame; its
    // batch is shared with it then, not copied.
    Ok(SharedTable::framed(Arc::unwrap_or_clone(frame), table, filters))
}

/// Whether `frame`'s key is one column of integers, whose numbers may be
/// dense in their span (RULES.md D601).
fn one_of_integers(frame: &Frame) -> bool {
    let [place] = frame.places.as_slice() else { return false };
    matches!(frame.right.columns.get(*place), Some(Column::Ints(_)))
}

/// The rows of `parts` as one batch, each vector joined by a thread of
/// `crew`'s.
fn joined(mut parts: Vec<Batch>, crew: Crew<'_>) -> Result<Batch> {
    if parts.len() < 2 {
        return Ok(parts.pop().unwrap_or_else(|| Batch { columns: Vec::new(), rows: 0 }));
    }
    let rows = parts.iter().fold(0_usize, |n, part| n.saturating_add(part.rows));
    let width = parts.first().map_or(0, |part| part.columns.len());
    let parts = Arc::new(parts);
    let columns = crew.run(width, Box::new(move |at| Ok(Batch::joined_at(&parts, at))))?;
    Ok(Batch { columns, rows })
}

/// The keys of `frame`'s rows hashed, a run of rows at a time, each by a
/// thread of `crew`'s: given as numbers, and what they say of a direct
/// table, if `numbers`.
fn hashed(frame: &Arc<Frame>, numbers: bool, crew: Crew<'_>) -> Result<Vec<Hashed>> {
    let (rows, each) = (frame.right.rows, crew.run_rows());
    let held = Arc::clone(frame);
    let work = Box::new(move |run: usize| -> Result<Hashed> {
        let from = run.saturating_mul(each);
        let given: Vec<usize> = (from..from.saturating_add(each).min(rows)).collect();
        let mut hashes = KeyHashes::default();
        hashes.of_batch((&held.right, Some(&given)), (&held.places, &held.nulls), held.hashing(numbers))?;
        let span = numbers.then(|| Span::of(&hashes.hashes, &hashes.misses));
        Ok((hashes, span))
    });
    crew.run(rows.div_ceil(each), work)
}

/// The rows `hashed` sorted by the parts of `parting`, a run at a time,
/// each by a thread of `crew`'s; keys given as `numbers` hashed first, if
/// the table is not direct.
fn parted(frame: &Arc<Frame>, hashed: &Arc<Vec<Hashed>>, (parting, numbers): (Parting, bool), crew: Crew<'_>) -> Result<Vec<Parted>> {
    let (held, each, seed) = (Arc::clone(hashed), crew.run_rows(), frame.seed.clone());
    let work = Box::new(move |run: usize| -> Result<Parted> {
        let Some((keys, _)) = held.get(run) else { return Ok(Parted::default()) };
        let base = run.saturating_mul(each);
        if numbers && !parting.direct() {
            return Ok(Parted::of(&parting, (&numbers_hashed(&keys.hashes, &seed), &keys.misses), base));
        }
        Ok(Parted::of(&parting, (&keys.hashes, &keys.misses), base))
    });
    crew.run(hashed.len(), work)
}

/// The table of `runs` as `parting` parts it, each part made by a thread
/// of `crew`'s, and beside them each of `frame`'s `keys` keys' runtime
/// filter, if filters are wanted (RULES.md D369).
fn made(frame: &Arc<Frame>, runs: &Arc<Vec<Parted>>, parting: Parting, (keys, crew): (usize, Crew<'_>)) -> Result<(JoinTable, Filters)> {
    let filtered = if wanted(crew.db) { keys } else { 0 };
    let (held, lists) = (Arc::clone(frame), Arc::clone(runs));
    let work = Box::new(move |n: usize| -> Result<Made> {
        match n.checked_sub(BUILD_PARTS) {
            None => Ok(Made::Part(parting.part(&lists, n))),
            Some(k) => Ok(Made::Filter(held.filter(k)?.map(Arc::new))),
        }
    });
    let (mut parts, mut filters) = (Vec::with_capacity(BUILD_PARTS), Vec::with_capacity(filtered));
    for made in crew.run(BUILD_PARTS.saturating_add(filtered), work)? {
        match made {
            Made::Part(part) => parts.push(part),
            Made::Filter(filter) => filters.push(filter),
        }
    }
    Ok((JoinTable::of_parts(&parting, parts, runs), wanted(crew.db).then_some(filters)))
}

// Unit tests: a table built by many threads against the one the
// statement's thread builds (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::{Crew, shared};
    use crate::db::Database;
    use crate::exec::Db;
    use crate::exec::batch::SharedTable;
    use crate::physical::JoinKey;
    use crate::storage::{Batch, Column, Ints, Mask, Texts};
    use crate::value::{Decimal, Value};

    /// Batches of `rows` rows each, `count` of them, of a key `key` gives
    /// of each row's number, null every 17th, the row's number, and a
    /// text of its number modulo 5.
    fn parts(count: usize, rows: usize, key: fn(i64) -> i64) -> Vec<Batch> {
        let part = |numbers: &[i64]| {
            let nulls: Vec<bool> = numbers.iter().map(|n| n % 17 == 0).collect();
            let keys = Ints::new(numbers.iter().map(|&n| key(n)).collect(), 0, Some(Mask::from_bools(&nulls)));
            let (mut text, mut ends) = (String::new(), Vec::new());
            for name in numbers.iter().map(|n| format!("t{}", n % 5)) {
                text.push_str(&name);
                ends.push(text.len());
            }
            let columns =
                vec![Column::Ints(keys), Column::Ints(Ints::new(numbers.to_vec(), 0, None)), Column::Texts(Texts::new(text, ends, None))];
            Batch { columns, rows }
        };
        let all: Vec<i64> = (0_i64..).take(count.saturating_mul(rows)).collect();
        all.chunks(rows).map(part).collect()
    }

    /// A database whose morsels hold 64 rows.
    fn database() -> Database {
        let db = Database::new();
        db.set_setting("EXECUTOR.MORSEL_ROWS", &Value::Exact(Decimal::new(64, 0))).unwrap();
        db
    }

    /// A case: how a row's key is made of its number, the join's keys, and
    /// whether their table is direct.
    type Case = (fn(i64) -> i64, Vec<JoinKey>, bool);

    /// The pairs a probe of every row of `probe` finds of `table` on
    /// `keys`, and how many of its keys have a runtime filter.
    fn seen(table: &SharedTable, probe: &Batch, keys: &[JoinKey]) -> (Vec<(usize, usize)>, usize) {
        let left: Vec<usize> = keys.iter().map(|k| k.left).collect();
        (table.matched(probe, &left).unwrap(), table.filters(keys).unwrap().len())
    }

    /// Verifies: [DESIGN] RULES.md D1080: a table built by many threads
    /// pairs every probing row with the rows the table the statement's
    /// thread builds pairs it with, in order, holds each row's values, and
    /// makes the runtime filters it makes: of keys dense in their span, direct; of keys
    /// apart, hashed; of two columns; of texts; on a key whose nulls
    /// match.
    #[test]
    fn a_table_built_by_many_threads_is_the_one_a_thread_builds() {
        let db = database();
        let state = Db::from(db.snapshot().unwrap());
        let crew = Crew { db: &state, tx: crate::txn::NO_TRANSACTION, degree: 4 };
        let seed = state.functions.seed().clone();
        let key = |left: usize, right: usize, nulls: bool| JoinKey { left, right, nulls };
        let cases: [Case; 5] = [
            (|n| n % 900, vec![key(0, 0, false)], true),
            (|n| (n % 900) * 1_000, vec![key(0, 0, false)], false),
            (|n| n % 900, vec![key(0, 0, false), key(2, 2, false)], false),
            (|n| n, vec![key(2, 2, false)], false),
            (|n| n % 900, vec![key(0, 0, true)], true),
        ];
        for (of_key, keys, direct) in cases {
            let layout = || ((0..3).collect::<Vec<usize>>(), 3);
            let alone = SharedTable::of(parts(40, 50, of_key), layout(), &keys, (seed.clone(), true)).unwrap();
            let together = shared(parts(40, 50, of_key), layout(), &keys, crew).unwrap();
            assert_eq!((alone.parts(), together.parts()), (1, crate::exec::table::BUILD_PARTS), "{keys:?}");
            assert_eq!((alone.is_direct(), together.is_direct(), together.rows()), (direct, direct, 2_000), "{keys:?}");
            let probe = parts(3, 400, of_key).remove(1);
            let (pairs, filters) = seen(&alone, &probe, &keys);
            assert!(!pairs.is_empty() && filters > 0, "{keys:?}");
            assert_eq!(seen(&together, &probe, &keys), (pairs, filters), "{keys:?}");
            let held = |table: &SharedTable| (0..2_000).step_by(97).map(|p| format!("{:?}", table.row_of(p))).collect::<Vec<_>>();
            assert_eq!(held(&together), held(&alone), "each row's values its own");
        }
    }

    /// Verifies: [DESIGN] RULES.md D1080 (MC/DC): a table is built by many
    /// threads only of more rows than a morsel holds, by a part of more
    /// than one thread. Decision: degree above 1 and rows above
    /// EXECUTOR.MORSEL_ROWS. Each alone leaves it to the statement's
    /// thread: degree 1, and exactly a morsel's rows.
    #[test]
    fn a_small_operand_or_one_thread_builds_alone() {
        let db = database();
        let state = Db::from(db.snapshot().unwrap());
        let keys = [JoinKey { left: 0, right: 0, nulls: false }];
        let built = |rows: usize, degree: usize| {
            let crew = Crew { db: &state, tx: crate::txn::NO_TRANSACTION, degree };
            shared(parts(1, rows, |n| n * 100), ((0..3).collect(), 3), &keys, crew).unwrap().parts()
        };
        assert_eq!(built(65, 2), crate::exec::table::BUILD_PARTS, "more than a morsel, two threads");
        assert_eq!(built(65, 1), 1, "one thread");
        assert_eq!(built(64, 2), 1, "a morsel's rows");
    }
}
