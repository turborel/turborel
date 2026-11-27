//! \[DESIGN\] The built operand of a hash join run in parallel (RULES.md
//! D292, D368): its rows read by its morsels on up to the degree's threads
//! if it is a part run in parallel, each morsel's gathered into a batch,
//! else by batch on the statement's thread if it gives batches (RULES.md
//! D1050), else row by row; then joined in order into one batch and put
//! in a join's table (`batch/joined/shared.rs`), which every morsel of
//! the probing pipeline then reads at once, as no thread writes it again.
//! Leis et al.,
//! "Morsel-Driven Parallelism" (2014), §4.2, build one table of every row.
//! An operand read by its morsels, of more rows than one holds, is put
//! together by those threads too (`parallel/build.rs`, RULES.md D1080):
//! its table in parts of the hashes' high bits, each part made by one
//! thread of its own rows, so no insertion is atomic, a thing the core
//! cannot make without `unsafe` (RULES.md R-SAFE-1). Any other is put
//! together by the statement's thread, a pass over the hashes.
//!
//! A row with a null key is left out, as none matches it [X3.135-1986
//! §5.11 GR2], but of a key on which a null matches a null [ISO/IEC
//! 9075-2:2023 §8.15], as the serial join leaves it out. The rows keep the
//! order the operand gives them, so a probing row's matches come in the
//! order they would alone. An operand no two rows of which share a key
//! may give them in any order (RULES.md D1221), a probing row then
//! matching one at most.

use std::sync::Arc;

use super::charged::{Taken, Tally, charged_batches, charged_rows};
use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{SharedTable, TABLE_ROW_BYTES, built_by_batch, layout, whole_keyed};
use crate::exec::{Runner, Seed, execute};
use crate::physical::{JoinKey, Plan};
use crate::storage::{Batch, Row};
use crate::txn::TxId;

/// \[DESIGN\] The partitions of a parallel grouping's and window's rows:
/// enough that each thread of any degree takes several, a power of two.
pub(super) const PARTS: usize = 64;

/// A hash join's built operand.
pub(super) struct Table {
    keys: Vec<JoinKey>,
    pub(super) shared: Arc<SharedTable>,
}

/// \[DESIGN\] The partition of the key `key`, the bytes of a group's
/// values, by its XXH3 seeded by the database's seed, so the morsels of a
/// query agree on each key's partition (RULES.md D357).
pub(super) fn part_of(key: &[u8], seed: &Seed) -> usize {
    let parts = u64::try_from(PARTS).unwrap_or(1);
    seed.xxh3(key).checked_rem(parts).and_then(|p| usize::try_from(p).ok()).unwrap_or_default()
}

impl Table {
    /// Gives `each` each row matching left row `l`, in order.
    ///
    /// # Errors
    ///
    /// As `each` fails, or a column a row does not have (SQLCODE -910).
    pub(super) fn each_match(&self, l: &Row, each: impl FnMut(&Row) -> Result<()>) -> Result<()> {
        self.shared.each_match(&self.keys, l, each)
    }

    /// The join's keys.
    pub(super) fn keys(&self) -> &[JoinKey] {
        &self.keys
    }
}

/// The batch of `rows`, each of `width` columns, every column a vector of
/// values.
fn batch_of(rows: &[Row], width: usize) -> Batch {
    let columns: Vec<usize> = (0..width).collect();
    let mut batch = Batch::empty(width);
    for row in rows {
        batch.push(row, &columns);
    }
    batch
}

/// The bytes a built batch holds, its table's share with it.
fn held_bytes(batch: &Batch) -> usize {
    batch.bytes().saturating_add(batch.rows.saturating_mul(TABLE_ROW_BYTES))
}

/// The operand `right` of a hash join on `keys`, built: by its morsels on
/// up to `degree` threads if it is a part run in parallel, else read by
/// the statement's thread; none if its rows outgrow `limit`, the join then
/// giving way to the serial one, which partitions them (RULES.md D38).
///
/// # Errors
///
/// As reading the operand fails.
pub(super) fn built(right: &Plan, keys: &[JoinKey], (db, tx): (&Db, TxId), limit: &Limit) -> Result<Held<Arc<Table>>> {
    if let Plan::Parallel(p) = right
        && pipelined(&p.input)
        && let Held::Kept(pipeline) = Pipeline::of(&p.input, (db, tx), limit)?
    {
        return parallel_built(pipeline, keys, (db, tx), (p.degree, limit));
    }
    // An operand that gives batches is read by batch, as the serial join
    // reads it (RULES.md D1050), a grouping run in parallel among them
    // (RULES.md D1221).
    if let Some(shape) = layout(right, db).filter(|_| crate::exec::batched(db) && built_by_batch(right, db)) {
        return batch_built(right, shape, keys, (db, tx), limit);
    }
    let mut rows = Vec::new();
    for read in execute(right.clone(), db, tx) {
        let row = read?;
        if !limit.take(crate::spill::row_bytes(&row).saturating_add(TABLE_ROW_BYTES)) {
            return Ok(Held::GaveWay);
        }
        rows.push(row);
    }
    let width = width_of(&rows, keys);
    let parts = vec![batch_of(&rows, width)];
    let shared =
        SharedTable::of(parts, ((0..width).collect(), width), keys, (db.functions.seed().clone(), crate::exec::batch::dense_wanted(db)))?;
    Ok(Held::Kept(Arc::new(Table { keys: keys.to_vec(), shared: Arc::new(shared.on(crate::db::kernels::chosen(db))) })))
}

/// \[DESIGN\] (RULES.md D1050) The operand `right` of a hash join on
/// `keys`, one that gives batches of `columns` of rows `width` wide, read
/// by batch on the statement's thread as the serial join reads it
/// (`stream::whole_keyed`): its scans judge the conditions told them
/// over their vectors and decode late the columns of the rows kept alone,
/// and each batch's kept rows are held as vectors of their kinds, no row
/// made of them. Every join built here finds its rows by their keys
/// alone, so a grouping keyed by all its columns is read in any order
/// (RULES.md D1221). Given way if they outgrow `limit`.
///
/// # Errors
///
/// As reading the operand fails.
fn batch_built(
    right: &Plan,
    (columns, width): (Vec<usize>, usize),
    keys: &[JoinKey],
    (db, tx): (&Db, TxId),
    limit: &Limit,
) -> Result<Held<Arc<Table>>> {
    let mut parts = Vec::new();
    for kept in whole_keyed(right.clone(), keys, db, tx)? {
        let (batch, rows) = kept?;
        let places = crate::storage::sel::places_of(rows, batch.rows);
        if places.is_empty() {
            continue;
        }
        let part = Batch::joined(vec![(batch, places)]);
        if !limit.take(held_bytes(&part)) {
            return Ok(Held::GaveWay);
        }
        parts.push(part);
    }
    let shared = SharedTable::of(parts, (columns, width), keys, (db.functions.seed().clone(), crate::exec::batch::dense_wanted(db)))?;
    Ok(Held::Kept(Arc::new(Table { keys: keys.to_vec(), shared: Arc::new(shared.on(crate::db::kernels::chosen(db))) })))
}

/// Whether `plan` is a pipeline its morsels read (`pipeline::branched`):
/// scans, filters, projections, counted operators and hash joins' left
/// operands, appended; any other part run in parallel, as a grouping,
/// is read whole.
pub(in crate::exec) fn pipelined(plan: &Plan) -> bool {
    match plan {
        Plan::SeqScan { .. } | Plan::IndexScan { .. } => true,
        Plan::Filter { input, .. } | Plan::Project { input, .. } => pipelined(input),
        Plan::HashJoin { left, .. } => pipelined(left),
        // A LEFT join's probe on its keys alone is a step (RULES.md D1076).
        Plan::OuterJoin { kind, left, keys, cond, own, .. } => {
            let on_keys = !keys.is_empty() && cond.is_none() && own.is_none();
            *kind == crate::logical::OuterJoin::Left && on_keys && pipelined(left)
        }
        // A hashed semi join's probe is a step (RULES.md D1075).
        Plan::SemiJoin(s) => s.method == crate::physical::SemiMethod::Hash && pipelined(&s.left),
        Plan::Append { left, right, .. } => pipelined(left) && pipelined(right),
        Plan::Counted(c) => pipelined(&c.input),
        // A phase reads its leaves whole before its chain (RULES.md D665).
        Plan::Transfer(_)
        | Plan::Nearest(_)
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::MarkJoin(_)
        | Plan::Product { .. }
        | Plan::Aggregate { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Shared(_)
        | Plan::Parallel(..) => false,
    }
}

/// The width of the operand's rows `rows`; of none, past its keys'
/// columns, as no row is read of it.
fn width_of(rows: &[Row], keys: &[JoinKey]) -> usize {
    rows.first().map_or_else(|| keys.iter().map(|k| k.right.saturating_add(1)).max().unwrap_or(0), Vec::len)
}

/// `pipeline`, a hash join's operand, built by its morsels on up to
/// `degree` threads, each morsel's rows gathered into a batch: of its
/// batches, kept rows alone, if it is read by batch, else of its rows.
/// Each morsel's rows are charged to `limit` as they are gathered
/// (RULES.md D1150), so the join gives way once they outgrow it, not
/// after a morsel whose rows a join below made many more than its table's.
fn parallel_built(
    pipeline: Pipeline,
    keys: &[JoinKey],
    (db, tx): (&Db, TxId),
    (degree, limit): (usize, &Limit),
) -> Result<Held<Arc<Table>>> {
    let pipeline = Arc::new(pipeline);
    let shape = pipeline.batched.as_ref().map(|b| (b.layout.clone(), b.width));
    let (count, db_for, read, within) = (pipeline.count(), db.clone(), Arc::clone(&pipeline), limit.clone());
    let batched_width = shape.as_ref().map(|s| s.1);
    let work = Box::new(move |m: usize| morsel_built(&read, m, (&db_for, tx), (batched_width, &within)));
    let (mut parts, from, mut taken) = (Vec::new(), limit.held(), 0_usize);
    for (done, batch) in (1_usize..).zip(Morsels::start(db, tx, (count, degree), work)) {
        let Taken::Whole(batch, bytes) = batch? else { return Ok(Held::GaveWay) };
        taken = taken.saturating_add(bytes);
        if !limit.on_course((from, taken), (done, count)) {
            return Ok(Held::GaveWay);
        }
        parts.push(batch);
    }
    let (columns, width) = shape.unwrap_or_else(|| {
        let widest = parts.iter().map(|b| b.columns.len()).max().unwrap_or_else(|| width_of(&[], keys));
        ((0..widest).collect(), widest)
    });
    // A morsel of no rows may hold no vectors.
    parts.retain(|b| b.rows > 0);
    // Put together by the threads that read it (RULES.md D1080).
    let shared = super::build::shared(parts, (columns, width), keys, super::build::Crew { db, tx, degree })?;
    Ok(Held::Kept(Arc::new(Table { keys: keys.to_vec(), shared: Arc::new(shared.on(crate::db::kernels::chosen(db))) })))
}

/// Morsel `m` of `pipeline`, a hash join's operand, gathered into a batch
/// charged to `limit` as it grows: of the kept rows of its batches if it
/// is read by batch, those of `width`, else of its rows; nothing if it
/// outgrows the grant.
///
/// # Errors
///
/// As reading the morsel fails.
fn morsel_built(
    pipeline: &Arc<Pipeline>,
    m: usize,
    (db, tx): (&Db, TxId),
    (width, limit): (Option<usize>, &Limit),
) -> Result<Taken<Batch>> {
    let tally = Tally::of(limit);
    let batch = match width {
        None => {
            let rows = charged_rows(pipeline.morsel_rows(m, (db, tx), Runner::new(db, tx))?, &tally).collect::<Result<Vec<Row>>>()?;
            batch_of(&rows, rows.first().map_or(0, Vec::len))
        }
        Some(width) => {
            let parts = charged_batches(pipeline.morsel_kept(m, (db, tx))?, &tally)
                .map(|k| {
                    k.map(|(b, rows)| {
                        let places = crate::storage::sel::places_of(rows, b.rows);
                        (b, places)
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            if parts.is_empty() || tally.past() { Batch::empty(width) } else { Batch::joined(parts) }
        }
    };
    // What the batch holds, its table's share with it, in place of what
    // its rows held as they were gathered.
    tally.settle(held_bytes(&batch));
    Ok(tally.taken(batch))
}

// Unit tests: a key's partition, and an operand built by batch (RULES.md
// R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::{Held, Limit, PARTS, built, part_of};
    use crate::db::Database;
    use crate::exec::{Db, Seed};
    use crate::logical::{Cond, Expr};
    use crate::physical::{JoinKey, Plan};
    use crate::value::{CompOp, Decimal, Value};

    /// A whole number, as a constant.
    fn number(n: i64) -> Expr {
        Expr::Const(Value::Exact(Decimal::new(i128::from(n), 0)))
    }

    /// The rows of table U of schema A whose K is at least 100.
    fn filtered_scan() -> Plan {
        let scan = Plan::SeqScan {
            table: crate::catalog::QualifiedName { auth: "A".into(), ident: "U".into() },
            columns: vec![0, 1],
            rows: 0,
            cost: 0.0_f64,
            order: Vec::new(),
            backward: false,
            most: None,
            located: false,
        };
        let cond = Cond::Compare(Expr::Column(0), CompOp::Ge, number(100));
        Plan::Filter { cond, detail: String::new(), est: 0.0_f64, of_read: false, input: Box::new(scan) }
    }

    /// The rows `plan` builds on its first column in `db`, and whether
    /// their table is direct, as one of a vector of integers alone is
    /// (RULES.md D601); none if it gave way.
    fn built_of(db: &Database, plan: &Plan) -> Option<(usize, bool)> {
        let state = Db::from(db.snapshot().unwrap());
        let keys = [JoinKey { left: 0, right: 0, nulls: false }];
        match built(plan, &keys, (&state, crate::txn::NO_TRANSACTION), &Limit::of(&state)).unwrap() {
            Held::Kept(table) => Some((table.shared.rows(), table.shared.is_direct())),
            Held::GaveWay => None,
        }
    }

    /// Verifies: [DESIGN] RULES.md D1050: a hash join's operand that is no
    /// part run in parallel is built of its batches' vectors, the rows its
    /// filter keeps alone, so keys dense in their span get a direct table
    /// (D601), which vectors of values never do; one past the work area
    /// gives way.
    ///
    /// Decision: batches are on, the operand gives batches, and its tables
    /// read by batch. Each alone sends it to the rows it gives one by one:
    /// EXECUTOR.BATCHES off, and an operand of values, which gives no
    /// batches; an operand whose tables do not read by batch is the
    /// grouping of `parallel::joins_build_by_batch`.
    #[test]
    fn an_operand_that_gives_batches_is_built_of_its_vectors() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE U (K INTEGER, N INTEGER)").unwrap();
        let a = db.session("A");
        let rows: Vec<String> = (0_i32..4_000_i32).map(|k| format!("({k}, {})", k % 7_i32)).collect();
        a.run(&format!("INSERT INTO U VALUES {}", rows.join(", "))).unwrap();
        a.commit().unwrap();
        assert_eq!(built_of(&db, &filtered_scan()), Some((3_900, true)), "by batch, the kept rows alone");
        let values = Plan::Values { rows: (0_i64..300_i64).map(|k| vec![number(k)]).collect(), detail: String::new() };
        assert_eq!(built_of(&db, &values), Some((300, false)), "no batches of values");
        db.set_setting("EXECUTOR.BATCHES", &Value::Char("OFF".into())).unwrap();
        assert_eq!(built_of(&db, &filtered_scan()), Some((3_900, false)), "batches off");
        db.set_setting("EXECUTOR.BATCHES", &Value::Char("ON".into())).unwrap();
        db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
        assert_eq!(built_of(&db, &filtered_scan()), None, "past the work area");
    }

    /// The scan of column K of table `t` of schema A.
    fn scan_of(t: &str) -> Box<Plan> {
        Box::new(Plan::SeqScan {
            table: crate::catalog::QualifiedName { auth: "A".into(), ident: t.into() },
            columns: vec![0],
            rows: 0,
            cost: 0.0_f64,
            order: Vec::new(),
            backward: false,
            most: None,
            located: false,
        })
    }

    /// Verifies: [DESIGN] RULES.md D1150, D38: a hash join's operand that
    /// is a part run in parallel, itself a join whose every row matches a
    /// hundred, is built of its morsels while their rows fit the grant;
    /// once a morsel outgrows it, the morsel is given up as it is gathered
    /// and the build gives way to the serial join, which partitions its
    /// rows to its spill area, so no morsel is held past the grant. By
    /// batch and row by row.
    ///
    /// Decision: the operand's morsels fit the grant, or not; they are
    /// read by batch, or row by row.
    #[test]
    fn a_build_of_morsels_past_its_grant_gives_way() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE TABLE U (K INTEGER)").unwrap();
        let a = db.session("A");
        let keys: Vec<String> = (0_i32..300_i32).map(|k| format!("({})", k % 3_i32)).collect();
        a.run(&format!("INSERT INTO T VALUES {}", keys.join(", "))).unwrap();
        a.run(&format!("INSERT INTO U VALUES {}", keys.join(", "))).unwrap();
        a.commit().unwrap();
        let join = Plan::HashJoin {
            left: scan_of("T"),
            right: scan_of("U"),
            keys: vec![JoinKey { left: 0, right: 0, nulls: false }],
            detail: String::new(),
            est: 0.0_f64,
        };
        let operand = Plan::Parallel(Box::new(crate::physical::Parallel { degree: 2, input: join }));
        let bytes = |n: i128| Value::Exact(Decimal::new(n, 0));
        db.set_setting("EXECUTOR.MORSEL_ROWS", &bytes(64)).unwrap();
        for batches in ["ON", "OFF"] {
            db.set_setting("EXECUTOR.BATCHES", &Value::Char(batches.into())).unwrap();
            db.set_setting("MEMORY.WORK_BYTES", &bytes(64_i128 << 20_u32)).unwrap();
            assert_eq!(built_of(&db, &operand).map(|b| b.0), Some(30_000), "within the grant, batches {batches}");
            db.set_setting("MEMORY.WORK_BYTES", &bytes(65_536)).unwrap();
            assert_eq!(built_of(&db, &operand), None, "past the grant, batches {batches}");
        }
    }

    /// Verifies: [DESIGN] RULES.md D357: a key's partition is the same each
    /// time a query's seed is asked it, so its morsels agree, and keys
    /// spread over the partitions; under another seed it is the XXH3 of
    /// that seed.
    #[test]
    fn a_key_keeps_its_partition_within_a_query() {
        let seed = Seed::fixed(0x5EED);
        let keys: Vec<Vec<u8>> = (0_u32..4_096).map(|k| k.to_le_bytes().to_vec()).collect();
        let first: Vec<usize> = keys.iter().map(|k| part_of(k, &seed)).collect();
        assert!(keys.iter().zip(&first).all(|(k, &p)| part_of(k, &seed) == p), "the same partition again");
        assert!(first.iter().all(|&p| p < PARTS));
        let mut used = first.clone();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used.len(), PARTS, "every partition takes keys");
        let other = Seed::fixed(0xD1FF);
        let moved = keys.iter().zip(&first).filter(|&(k, &p)| part_of(k, &other) != p).count();
        assert!(moved * 2 > keys.len(), "another seed parts the keys anew, {moved} moved");
    }
}
