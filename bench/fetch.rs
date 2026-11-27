//! What one fetch of a row by its row id costs (RULES.md D31, D862): a
//! table of an order entry's stock, 100,000 rows of 17 columns under a
//! unique index of two integers, every page in the pool. One integer
//! column is read by row id through `TableStorage::get_listed` with a
//! `Near` held from read to read, for row ids scattered and in key order,
//! then behind a probe of the unique index, outside a transaction, and in
//! transactions of a few reads each whose reads are not checked (READ
//! COMMITTED) and are (SERIALIZABLE). On one thread and on eight. Each
//! case's rate is also noted as nanoseconds a fetch on each thread.
//! TURBOREL_FETCH names a case to run it alone, and TURBOREL_FETCH_THREADS
//! a count of threads, for a profile. Then the same table of the column
//! store, two groups of 65,536 rows, every row frozen and every header
//! held: one column, four, eight and all seventeen read by scattered row
//! ids, and one in key order, on one thread. A frozen fetch of each count
//! of columns against a fetch of the tree's is what the planner's
//! `FROZEN_FETCH` and `FROZEN_FETCH_COLUMN` price
//! (`engine/rows/frozen.rs`, RULES.md D1091, D1097). Run with `cargo xtask bench`, on the harness (RULES.md
//! R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use harness::{Bench, Case, on_threads, scatter};
use turborel::catalog::{ColumnDesc, QualifiedName, TableDesc};
use turborel::disk::buffer::{BufferPool, Table, Weighted};
use turborel::engine::rows::{RowStore, TableFiles, TableOptions};
use turborel::index::{BTreeMethod, Index, IndexMethod, Key};
use turborel::log::MemoryLog;
use turborel::storage::{Near, Row, RowId, StorageEngine, TableStorage};
use turborel::txn::{NO_TRANSACTION, ReadMode, TxId};
use turborel::value::Spelled;
use turborel::value::{DataType, Decimal, Value};

/// Rows of the table: a warehouse's STOCK.
const ROWS: u64 = 100_000;
/// Frames: enough for the table and its index.
const FRAMES: usize = 32_768;
/// Thread counts.
const THREADS: [usize; 2] = [1, 8];
/// Passes over the rows a thread makes in one run.
const PASSES: u64 = 4;
/// Reads of one transaction.
const READS: u64 = 16;
/// The column read: S_QUANTITY, an integer.
const READ: [usize; 1] = [2];
/// Four columns read: S_QUANTITY, a S_DIST, S_YTD and S_DATA.
const READ_FOUR: [usize; 4] = [2, 5, 13, 16];
/// Eight columns read: those four, three more S_DIST and S_ORDER_CNT.
const READ_EIGHT: [usize; 8] = [2, 3, 5, 7, 9, 13, 14, 16];
/// Every column read.
const READ_ALL: [usize; 17] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
/// Rows of the table of the column store: two groups whole, so none is
/// left in its tree.
const FROZEN_ROWS: u64 = 2 * 65_536;
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };

/// How a case's reads are made.
#[derive(Clone, Copy)]
enum How {
    /// Outside any transaction.
    Bare,
    /// In transactions of `READS` reads, checked or not.
    In { checked: bool },
}

/// The table, its unique index, and its rows' ids in key order.
struct Stock {
    _store: RowStore,
    table: Arc<dyn TableStorage>,
    index: Arc<dyn Index>,
    ids: Vec<RowId>,
    /// How many rows it holds.
    rows: u64,
    /// The next transaction's number.
    next: AtomicU64,
}

fn main() {
    let bench = Bench::new("fetch");
    let only = std::env::var("TURBOREL_FETCH").ok();
    let stock = stock(&bench, ROWS, false);
    let one = std::env::var("TURBOREL_FETCH_THREADS").ok().and_then(|t| t.parse::<usize>().ok());
    for threads in THREADS.into_iter().filter(|&t| one.is_none_or(|one| one == t)) {
        let cases: [(&str, bool, bool, How); 6] = [
            ("row id, scattered", true, false, How::Bare),
            ("row id, in key order", false, false, How::Bare),
            ("index probe and fetch", true, true, How::Bare),
            ("row id, scattered, READ COMMITTED", true, false, How::In { checked: false }),
            ("index probe and fetch, READ COMMITTED", true, true, How::In { checked: false }),
            ("index probe and fetch, SERIALIZABLE", true, true, How::In { checked: true }),
        ];
        for (name, scattered, probed, how) in cases {
            if only.as_deref().is_some_and(|o| o != name) {
                continue;
            }
            let total = threads as u64 * ROWS * PASSES;
            bench.measure(Case::new(name, "fetches").threads(threads), || {
                on_threads(threads, |t| stock.read(t, (scattered, probed), how, &READ));
                total
            });
            if let Some(rate) = bench.rate(name, threads) {
                harness::note(&format!("{name}, {threads} threads: {:.0} ns a fetch", threads as f64 * 1e9_f64 / rate));
            }
        }
    }
    frozen(&bench, only.as_deref());
}

/// The cases of the table of the column store, on one thread.
fn frozen(bench: &Bench, only: Option<&str>) {
    let stock = stock(bench, FROZEN_ROWS, true);
    let cases: [(&str, bool, &[usize]); 5] = [
        ("row id, scattered, column store", true, &READ),
        ("row id, in key order, column store", false, &READ),
        ("row id, scattered, four columns, column store", true, &READ_FOUR),
        ("row id, scattered, eight columns, column store", true, &READ_EIGHT),
        ("row id, scattered, every column, column store", true, &READ_ALL),
    ];
    for (name, scattered, columns) in cases {
        if only.is_some_and(|o| o != name) {
            continue;
        }
        bench.measure(Case::new(name, "fetches"), || {
            stock.read(0, (scattered, false), How::Bare, columns);
            FROZEN_ROWS * PASSES
        });
        if let Some(rate) = bench.rate(name, 1) {
            harness::note(&format!("{name}: {:.0} ns a fetch", 1e9_f64 / rate));
        }
    }
}

impl Stock {
    /// Thread `t`'s reads of one run: every row `PASSES` times, scattered
    /// or in key order, by its id or behind a probe of the index. Not
    /// inlined, so a profile names it.
    #[inline(never)]
    fn read(&self, t: usize, (scattered, probed): (bool, bool), how: How, columns: &[usize]) {
        let (mut near, mut out, mut bytes) = (Near::default(), Row::new(), Vec::new());
        let by = self.index.key_probe().unwrap();
        let (_table, _index) = (self.table.pin(), self.index.pin());
        let mut tx = NO_TRANSACTION;
        for n in 0..self.rows * PASSES {
            if let How::In { checked } = how
                && n % READS == 0
            {
                self.end(tx);
                tx = self.begin(checked);
            }
            let at = if scattered { scatter(n + t as u64 * 1009, self.rows) } else { n % self.rows };
            out.clear();
            let found = if probed {
                assert!(by.whole_key(&[number(1), number(at)], &mut bytes));
                let mut found = false;
                by.entries_of(&bytes, &mut |entry| {
                    found = self.table.get_listed(tx, entry.row, columns, (&mut near, &mut out))?;
                    Ok(false)
                })
                .unwrap();
                found
            } else {
                self.table.get_listed(tx, self.ids[at as usize], columns, (&mut near, &mut out)).unwrap()
            };
            assert!(found && out.len() == columns.len());
        }
        self.end(tx);
    }

    /// A transaction begun in the table, its reads checked if `checked`.
    fn begin(&self, checked: bool) -> TxId {
        let tx = self.next.fetch_add(1, Ordering::Relaxed);
        self.table.begin(tx, tx).unwrap();
        if !checked {
            self.table.read_mode(tx, ReadMode { snapshot: tx, checked }).unwrap();
        }
        tx
    }

    /// Ends transaction `tx`, if it is one.
    fn end(&self, tx: TxId) {
        if tx != NO_TRANSACTION {
            self.table.rollback(tx).unwrap();
        }
    }
}

fn number(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The key of item `n`: its warehouse and its number.
fn key(n: u64) -> Key {
    Key(vec![number(1), number(n)])
}

/// Row `n`: S_I_ID, S_W_ID, S_QUANTITY, ten S_DIST of 24 characters,
/// S_YTD, S_ORDER_CNT, S_REMOTE_CNT and S_DATA.
fn row(n: u64) -> Row {
    let mut row = vec![number(n), number(1), number(10 + n % 91)];
    row.extend((0..10_u32).map(|d| Value::Char(format!("{:<24}", format!("dist {d} of {n}")).into())));
    row.extend([number(0), number(0), number(0), Value::Char(format!("{:<40}", format!("stock data {n}")).into())]);
    row
}

/// The table loaded with `rows` rows, each row's index entry written
/// after it by the thread that wrote the row, as a statement does, so the
/// entry carries the row's leaf as its hint; of the column store if
/// `frozen`, its rows then frozen into groups.
fn stock(bench: &Bench, rows: u64, frozen: bool) -> Stock {
    let pool = Arc::new(BufferPool::new(FRAMES, Box::new(Arc::new(Weighted::new(FRAMES, Arc::new(Table::default()))))).unwrap());
    let file = pool.add_pager(Arc::new(bench.pager(if frozen { "fetch-frozen" } else { "fetch" })));
    let files = TableFiles::new(Arc::clone(&pool), file, Arc::new(MemoryLog::new()));
    let store = if frozen { RowStore::columns(files, TableOptions::default()) } else { RowStore::new(files, TableOptions::default()) };
    let mut columns = vec![("S_I_ID".to_owned(), INTEGER), ("S_W_ID".to_owned(), INTEGER), ("S_QUANTITY".to_owned(), INTEGER)];
    columns.extend((1..=10_u32).map(|d| (format!("S_DIST_{d:02}"), DataType::character(24))));
    columns.extend(["S_YTD", "S_ORDER_CNT", "S_REMOTE_CNT"].map(|name| (name.to_owned(), INTEGER)));
    columns.push(("S_DATA".to_owned(), DataType::character(50)));
    let desc = TableDesc {
        periods: Vec::new(),
        name: QualifiedName { auth: "A".into(), ident: "STOCK".into() },
        columns: columns
            .into_iter()
            .map(|(name, data_type)| ColumnDesc {
                name,
                data_type,
                not_null: false,
                default: None,
                domain: None,
                collation: None,
                generated: None,
            })
            .collect(),
        constraints: vec![],
        engine: store.name().into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
        typed: None,
    };
    let table: Arc<dyn TableStorage> = store.create_table(&desc).unwrap().into();
    let index: Arc<dyn Index> = Arc::from(BTreeMethod::new(pool, file).create(&[INTEGER, INTEGER]).unwrap());
    let ids = (0..rows)
        .map(|n| {
            let id = table.insert(NO_TRANSACTION, row(n)).unwrap();
            index.insert(key(n), id).unwrap();
            id
        })
        .collect();
    // Every row was written outside a transaction, so committed before
    // every snapshot, and frozen as its group is due.
    while frozen && table.maintain(1, true).unwrap() {}
    Stock { _store: store, table, index, ids, rows, next: AtomicU64::new(1 << 40) }
}
