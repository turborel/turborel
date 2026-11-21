//! \[DESIGN\] Window functions, EXCEPT and INTERSECT run in parallel
//! (RULES.md D295): their rows split by morsels into partitions of a key,
//! a window's partitioning or a set operation's compared columns, so that
//! rows a function or a comparison takes together share one; then each
//! partition run by the serial operator on a thread of its own, its rows
//! in the order they came. No partition's result depends on another's
//! rows [ISO/IEC 9075-2:2023 §7.15 GR3; ISO/IEC 9075:1992 §7.10 GR1b].
//!
//! A partition's rows are held in a list that spills, each of its share
//! of a work area, and its operator runs in its share too, so the parts
//! together hold one work area (RULES.md D38), each spilling on its own.
//! The partitions' rows are merged back in the order the operator alone
//! gives: a window's by its sort keys, its partitions' rows never tied;
//! a set operation's by the place of each first operand's row, each
//! carried through it after the columns it compares.

use std::sync::{Arc, Mutex, PoisonError};

use super::pipeline::Pipeline;
use super::probe::PARTS;
use super::run::Morsels;
use super::sorted::merge_by;
use super::{Held, Limit};
use crate::db::State;
use crate::error::Result;
use crate::exec::{HeldRows, RowIter, Runner, TransitionRows, broken, execute};
use crate::logical::SortKey;
use crate::physical::Plan;
use crate::settings::{MEMORY_WORK_BYTES, SPILL_BATCH_BYTES, batch_bytes};
use crate::spill::{ListSpace, norm_row};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::{Decimal, Value};

/// \[DESIGN\] The bits of a row's place its place in its morsel takes:
/// more than any morsel's rows need.
const PLACE_BITS: u32 = 32;

/// The rows of each partition, held in lists that spill.
type Parts = Vec<HeldRows<Row>>;

/// [ISO/IEC 9075-2:2023 §7.15] The rows of the window `plan`, over a sort
/// of a stream by its partitioning, each partition's window functions run
/// on a thread of up to `degree`.
///
/// # Errors
///
/// As reading the stream fails.
pub(super) fn windowed(plan: &Plan, (db, tx): (&Arc<State>, TxId), degree: usize) -> Result<Held<RowIter>> {
    let Plan::Window { window, detail, .. } = plan else { return Err(broken("not a window run in parallel")) };
    let Plan::Sort { keys, input, .. } = &window.input else { return Err(broken("a window run in parallel reads no sort")) };
    let columns = window.partition.clone();
    let key = move |row: &Row| columns.iter().map(|&c| row.get(c).cloned()).collect();
    let limit = Limit::of(db);
    let Held::Kept((parts, _)) = split(input, (db, tx, degree), (key, false), (&limit, limit.most()))? else { return Ok(Held::GaveWay) };
    let (shape, detail, keys) = (Arc::new(window.with_input(|_| ())), detail.clone(), keys.clone());
    let sort_keys = keys.clone();
    let each = move |rows: TransitionRows| {
        let sorted = Plan::Sort { keys: sort_keys.clone(), detail: String::new(), input: Box::new(Plan::Transition { rows }), most: None };
        Plan::Window { window: Box::new(shape.with_input(|()| sorted)), detail: detail.clone(), est: 0.0_f64 }
    };
    let outs = per_part(parts, (db, tx, degree), (each, limit.most()))?;
    Ok(Held::Kept(limit.keeping(Box::new(merge_by(&outs, &keys).into_iter().map(Ok)))))
}

/// [ISO/IEC 9075:1992 §7.10 GR1b] The rows of EXCEPT or INTERSECT, `plan`,
/// of streams, each partition of the rows of its compared columns run on a
/// thread of up to `degree`.
///
/// # Errors
///
/// As reading the operands fails.
pub(super) fn set_operation(plan: &Plan, (db, tx): (&Arc<State>, TxId), degree: usize) -> Result<Held<RowIter>> {
    let Plan::SetOp { op, all, compared, left, right, .. } = plan else { return Err(broken("not a set operation run in parallel")) };
    let n = *compared;
    let key = move |row: &Row| Some(row.get(..n.unwrap_or(row.len()).min(row.len()))?.to_vec());
    // One grant, half of it each operand's.
    let limit = Limit::of(db);
    let half = limit.most() >> 1_u32;
    let Held::Kept((rights, _)) = split(right, (db, tx, degree), (key, false), (&limit, half))? else { return Ok(Held::GaveWay) };
    let Held::Kept((parts, width)) = split(left, (db, tx, degree), (key, true), (&limit, half))? else { return Ok(Held::GaveWay) };
    // No row of the first operand: none is kept.
    let Some(width) = width else { return Ok(Held::Kept(Box::new(std::iter::empty()))) };
    let (op, all, compared_of) = (*op, *all, Some(n.unwrap_or(width)));
    let rights: Arc<Vec<Mutex<Option<HeldRows<Row>>>>> = Arc::new(rights.into_iter().map(|r| Mutex::new(Some(r))).collect());
    let each = move |rows: TransitionRows, p: usize| {
        let taken = rights.get(p).and_then(|r| r.lock().unwrap_or_else(PoisonError::into_inner).take());
        let built = Box::new(Plan::Transition { rows: TransitionRows::with(taken.unwrap_or_else(|| HeldRows::new(ListSpace::default()))) });
        Plan::SetOp { op, all, compared: compared_of, left: Box::new(Plan::Transition { rows }), right: built, est: 0.0_f64 }
    };
    let outs = per_part_numbered(parts, (db, tx, degree), (each, limit.most()))?;
    let place = SortKey::new(width, crate::value::SortOrder::of(false, None));
    let rows = merge_by(&outs, &[place]).into_iter().map(move |mut row| {
        row.truncate(width);
        Ok(row)
    });
    Ok(Held::Kept(limit.keeping(Box::new(rows))))
}

/// The partition of a row whose key's normal bytes are `key`: of their
/// MurmurHash3, a vetted hash (RULES.md D357), so the partitions do not
/// depend on a hasher's seed or version.
fn part_of_key(key: &[u8]) -> usize {
    let (low, _) = crate::hash::murmur::murmur3_x64_128(key, 0);
    usize::try_from(low.checked_rem(u64::try_from(PARTS).unwrap_or(u64::MAX)).unwrap_or_default()).unwrap_or_default()
}

/// A list of `share` bytes of `db`'s work areas.
fn space(db: &State, share: usize) -> ListSpace {
    ListSpace {
        limit: share,
        batch: batch_bytes(db.settings.get(SPILL_BATCH_BYTES)),
        areas: Some(Arc::clone(&db.work)),
        types: db.types.clone(),
    }
}

/// The rows of the stream `input`, read by morsels on up to `degree`
/// threads, split into partitions by `key` of each, a row of none in the
/// first, each followed by its place among the stream's rows if
/// `numbered`; and the width of its rows before it, if it has one. Held
/// in `bytes` of `limit`'s grant, operands it builds within it. Given way
/// if a table cannot be split.
fn split(
    input: &Plan,
    (db, tx, degree): (&Arc<State>, TxId, usize),
    (key, numbered): (impl Fn(&Row) -> Option<Row> + Send + Sync + 'static, bool),
    (limit, bytes): (&Limit, usize),
) -> Result<Held<(Parts, Option<usize>)>> {
    let Held::Kept(pipeline) = Pipeline::of(input, (db, tx), limit)? else { return Ok(Held::GaveWay) };
    let (pipeline, db_for) = (Arc::new(pipeline), Arc::clone(db));
    let count = pipeline.count();
    let work = Box::new(move |m: usize| {
        let mut parts: Vec<Vec<Row>> = (0..PARTS).map(|_| Vec::new()).collect();
        let mut width = None;
        let base = i128::try_from(m).unwrap_or(i128::MAX).checked_shl(PLACE_BITS).unwrap_or(i128::MAX);
        for (place, mut row) in (0_i128..).zip(pipeline.morsel(m, &Runner::new(&db_for, tx))?) {
            let p = key(&row).map_or(0, |k| part_of_key(&norm_row(&k)));
            width = width.or(Some(row.len()));
            if numbered {
                row.push(Value::Exact(Decimal::new(base.saturating_add(place), 0)));
            }
            parts.get_mut(p).ok_or_else(|| broken("a partition of a parallel query is missing"))?.push(row);
        }
        Ok((parts, width))
    });
    let share = bytes.checked_div(PARTS).unwrap_or_default().max(1);
    let mut held: Parts = (0..PARTS).map(|_| HeldRows::new(space(db, share))).collect();
    let mut width = None;
    for made in Morsels::start(db, tx, (count, degree), work) {
        let (parts, w) = made?;
        width = width.or(w);
        for (list, rows) in held.iter_mut().zip(parts) {
            for row in rows {
                list.push(row)?;
            }
        }
    }
    Ok(Held::Kept((held, width)))
}

/// Runs the plan `each` makes of each partition's rows on a thread of up
/// to `degree`, each in a share of `bytes`: their rows, by partition.
fn per_part(
    parts: Parts,
    (db, tx, degree): (&Arc<State>, TxId, usize),
    (each, bytes): (impl Fn(TransitionRows) -> Plan + Send + Sync + 'static, usize),
) -> Result<Vec<Vec<Row>>> {
    per_part_numbered(parts, (db, tx, degree), (move |rows, _| each(rows), bytes))
}

/// As `per_part`, `each` told the partition's number.
fn per_part_numbered(
    parts: Parts,
    (db, tx, degree): (&Arc<State>, TxId, usize),
    (each, bytes): (impl Fn(TransitionRows, usize) -> Plan + Send + Sync + 'static, usize),
) -> Result<Vec<Vec<Row>>> {
    let shared = shared(db, bytes.checked_div(degree.max(1)).unwrap_or_default().max(1));
    let parts: Arc<Vec<Mutex<Option<HeldRows<Row>>>>> = Arc::new(parts.into_iter().map(|p| Mutex::new(Some(p))).collect());
    let work = Box::new(move |p: usize| {
        let rows = parts
            .get(p)
            .and_then(|r| r.lock().unwrap_or_else(PoisonError::into_inner).take())
            .ok_or_else(|| broken("a partition run twice"))?;
        execute(each(TransitionRows::with(rows), p), &shared, tx).collect::<Result<Vec<Row>>>()
    });
    Morsels::start(db, tx, (PARTS, degree), work).collect()
}

/// A copy of `db` whose operators are each granted `work` bytes of the
/// statement's grant, not of the budget again, for a part of a statement
/// run beside others in its grant (RULES.md D298).
fn shared(db: &Arc<State>, work: usize) -> Arc<State> {
    let mut copy = State::clone(db);
    copy.work = Arc::new(db.work.covered());
    copy.settings.set_key(MEMORY_WORK_BYTES, Value::Exact(Decimal::new(i128::try_from(work).unwrap_or(i128::MAX), 0)));
    Arc::new(copy)
}
