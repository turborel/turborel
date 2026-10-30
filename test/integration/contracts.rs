//! Contract tests for the extension traits (RULES.md R-SAFE-3). Each suite
//! checks one trait's documented contract against every built-in
//! implementation, and can check a plug-in the same way.

use std::sync::Arc;

use turborel::catalog::{
    Actions, Catalog, ColumnDesc, Horizon, IndexDesc, MemoryCatalog, PrivilegeDesc, QualifiedName, TableDesc, ViewDesc,
};
use turborel::db::{CatalogFiles, CatalogStorage, CatalogStore, Database, FileFormats, FileLogs, LogStorage, StorePlace};
use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::Pager;
use turborel::engine::rows::{RowStore, TableFiles, TableOptions};
use turborel::error::{self, Code};
use turborel::functions::ScalarFunction;
use turborel::index::HashMethod;
use turborel::log::{Log, MemoryLog};
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::NO_TRANSACTION;
use turborel::value::{DataType, Value};

use crate::common::{Scratch, crc32c, number};
use turborel::value::Spelled;

/// A row of the contract table: an integer and a CHARACTER(3).
fn row(n: i128, text: &str) -> Row {
    vec![number(n), Value::Char(text.into())]
}

/// The rows as text, for comparison.
fn shown(rows: &[(RowId, Row)]) -> Vec<(RowId, String)> {
    rows.iter().map(|(id, r)| (*id, r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","))).collect()
}

fn table_desc(auth: &str, ident: &str, engine: &str) -> TableDesc {
    let columns = vec![
        ColumnDesc {
            name: "N".into(),
            data_type: DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric },
            not_null: true,
            default: None,
            domain: None,
        },
        ColumnDesc { name: "S".into(), data_type: DataType::Character { length: 3 }, not_null: false, default: None, domain: None },
    ];
    TableDesc {
        name: QualifiedName { auth: auth.into(), ident: ident.into() },
        columns,
        constraints: vec![],
        engine: engine.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// Every live row of `table`, by id.
fn live(table: &Arc<dyn TableStorage>) -> Vec<(RowId, String)> {
    let mut rows: Vec<(RowId, Row)> = Arc::clone(table).scan(NO_TRANSACTION, &[0, 1]).collect::<error::Result<_>>().unwrap();
    rows.sort_by_key(|(id, _)| *id);
    shown(&rows)
}

/// Checks the `TableStorage` contract on a table of `engine`.
fn storage_contract(engine: &dyn StorageEngine) {
    let table: Arc<dyn TableStorage> = engine.create_table(&table_desc("A", "T", engine.name())).unwrap().into();
    let tx = NO_TRANSACTION;
    assert_eq!((table.row_count(), live(&table)), (0, vec![]), "a new table is empty");
    let a = table.insert(tx, row(1, "ONE")).unwrap();
    let b = table.insert(tx, row(2, "TWO")).unwrap();
    assert_ne!(a, b);
    assert_eq!(live(&table), vec![(a, "1,ONE".into()), (b, "2,TWO".into())]);
    table.update(tx, a, row(3, "THR")).unwrap();
    table.delete(tx, b).unwrap();
    assert_eq!((table.row_count(), live(&table)), (1, vec![(a, "3,THR".into())]));
    assert!(table.get(tx, b).unwrap().is_none(), "a deleted row is gone");
    assert!(table.update(tx, b, row(9, "NOT")).is_err(), "update of a deleted row fails");
    assert!(table.delete(tx, b).is_err(), "delete of a deleted row fails");
    assert_eq!(live(&table), vec![(a, "3,THR".into())], "and changes nothing");
    let c = table.insert(tx, row(4, "FOU")).unwrap();
    assert!(c != a && c != b, "an insert never reuses a deleted row's id");
    table.restore(tx, b, row(2, "TWO")).unwrap();
    assert_eq!((table.row_count(), live(&table)), (3, vec![(a, "3,THR".into()), (b, "2,TWO".into()), (c, "4,FOU".into())]));
    for cost in [table.scan_cost(&[0]), table.fetch_cost()] {
        assert!(cost.is_finite() && cost >= 0.0, "cost {cost}");
    }
}

/// Verifies: [DESIGN] `StorageEngine` and `TableStorage` contracts, for the
/// built-in row store, in memory and on files, the column store over its
/// files (RULES.md D31), and the test engine of coordinator-isolated
/// tables the other tests use.
#[test]
fn the_built_in_engine_keeps_the_storage_contract() {
    storage_contract(&RowStore::in_memory(TableOptions::default()));
    storage_contract(&crate::common::LockingEngine);
    let scratch = Scratch::new("contract-rowstore");
    let pager = Pager::create(&OsFileSystem, &scratch.path("rows"), crc32c(), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    storage_contract(&RowStore::new(files.clone(), TableOptions::default()));
    storage_contract(&RowStore::columns(files, TableOptions::default()));
}

/// Verifies: [DESIGN] `Catalog` contract, for `MemoryCatalog`.
#[test]
fn memory_catalog_keeps_the_catalog_contract() {
    let mut catalog = MemoryCatalog::default();
    assert!(!catalog.schema_exists("A"));
    catalog.create_schema("A", "A", vec![table_desc("A", "T", "rowstore"), table_desc("A", "U", "rowstore")]);
    assert!(catalog.schema_exists("A"));
    catalog.create_schema("B", "B", Vec::new());
    assert_eq!(catalog.schemas(), vec!["A", "B"], "in the order created");
    let names = |c: &MemoryCatalog| c.tables().iter().map(|t| t.name.to_string()).collect::<Vec<_>>();
    assert_eq!(names(&catalog), vec!["A.T", "A.U"]);
    assert_eq!(names(&catalog), names(&catalog), "a stable order");
    let t = QualifiedName { auth: "A".into(), ident: "T".into() };
    let u = QualifiedName { auth: "A".into(), ident: "U".into() };
    assert!(catalog.table(&t).is_some() && catalog.table(&u).is_some());
    for (name, table) in [("T_N", &t), ("U_N", &u)] {
        catalog.add_index(IndexDesc {
            name: name.into(),
            table: table.clone(),
            columns: vec![0],
            method: "btree".into(),
            unique: false,
            options: Vec::new(),
        });
    }
    assert_eq!(catalog.index("T_N").map(|i| i.table.to_string()), Some("A.T".into()));
    assert_eq!(catalog.indexes(&t).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["T_N"]);
    catalog.remove_index("U_N");
    assert!(catalog.index("U_N").is_none() && catalog.indexes(&u).is_empty(), "removed");
    assert!(catalog.index("T_N").is_some(), "the others stay");
    let grant = |table: &QualifiedName, grantee: &str| PrivilegeDesc {
        grantor: "A".into(),
        grantee: Some(grantee.into()),
        table: table.clone(),
        actions: Actions { select: true, ..Actions::default() },
        grantable: false,
    };
    let copy = catalog.duplicate();
    catalog.grant(vec![grant(&t, "B"), grant(&u, "C")]);
    catalog.grant(vec![grant(&t, "D")]);
    let grantees = |c: &dyn Catalog, table| c.privileges(table).iter().map(|p| p.grantee.clone()).collect::<Vec<_>>();
    assert_eq!(grantees(&catalog, &t), vec![Some("B".into()), Some("D".into())], "in the order granted");
    assert_eq!(grantees(&catalog, &u), vec![Some("C".into())]);
    assert!(grantees(&*copy, &t).is_empty(), "the duplicate is independent");
    let v = QualifiedName { auth: "A".into(), ident: "V".into() };
    let view = ViewDesc {
        name: v.clone(),
        columns: table_desc("A", "V", "").columns,
        query: "SELECT * FROM T".into(),
        names: "A".into(),
        check_option: None,
        grouped: false,
        updates: Some((t.clone(), vec![0])),
        reads: vec![t.clone()],
        horizon: Horizon::default(),
        underlying: None,
    };
    catalog.add_views(vec![view]);
    assert_eq!(catalog.view(&v).map(|w| w.query.clone()), Some("SELECT * FROM T".into()));
    assert!(catalog.table(&v).is_none() && !names(&catalog).contains(&"A.V".to_owned()), "a view is not a table");
    assert!(copy.view(&v).is_none(), "the duplicate is independent");
    catalog.create_schema("C", "Z", Vec::new());
    assert_eq!((catalog.schema_owner("A"), catalog.schema_owner("C"), catalog.schema_owner("D")), (Some("A"), Some("Z"), None));
    catalog.remove_schema("B");
    assert!(!catalog.schema_exists("B"));
    assert_eq!(catalog.schemas(), vec!["A", "C"], "the others as before");
}

/// A scalar function that breaks the contract: it calls back into the
/// database it runs in.
struct CallsBack(Database);
impl ScalarFunction for CallsBack {
    fn name(&self) -> &'static str {
        "CALLS_BACK"
    }
    fn return_type(&self, _: &[DataType]) -> error::Result<DataType> {
        Ok(DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric })
    }
    fn call(&self, _: &[Value]) -> error::Result<Value> {
        self.0.session("APP").run("INSERT INTO T VALUES (9, 'X', 1, 1)")?;
        Ok(number(1))
    }
}

/// Verifies: [DESIGN] RULES.md R-ERR-4, D27: a plug-in that calls back into
/// another session runs a separate transaction, which conflicts with the
/// caller's locks (SQLCODE -911) instead of panicking or hanging; a change of
/// definitions from inside one is refused (SQLCODE -910); reading from inside
/// a change is allowed; and the database stays usable.
///
/// Regression test (RULES.md R-SAFE-3): a call back was
/// documented as forbidden but panicked on a double borrow.
#[test]
fn calls_back_into_the_database_do_not_panic_or_hang() {
    let db = crate::common::locking_setup();
    db.register_scalar_function(Box::new(CallsBack(db.clone()))).unwrap();
    let mut s = db.session("APP");
    s.enable_extensions(true);
    assert_eq!(crate::common::code(&s, "SELECT CALLS_BACK(ID) FROM T"), Code::Conflict);
    // Waiting would never end: the holder runs on the same thread.
    db.set_lock_wait(turborel::txn::LockWait::Wait).unwrap();
    assert_eq!(crate::common::code(&s, "SELECT CALLS_BACK(ID) FROM T"), Code::Conflict, "refused, not hung");
    let nested = db.with_optimizer(|_| db.with_optimizer(|_| ()).map_err(|e| e.code)).unwrap();
    assert_eq!(nested, Err(Code::Api));
    assert!(db.with_optimizer(|_| db.active_transactions().is_ok()).unwrap());
    s.rollback().unwrap();
    assert_eq!(crate::common::count(&s, "SELECT ID FROM T"), 3, "nothing was inserted, and the database still works");
}

/// Verifies: [DESIGN] the `CostModel` invariant: a weight that is NaN,
/// infinite or negative is refused with SQLCODE -910, and the weights are
/// left as they were. Finite, non-negative weights are kept.
#[test]
fn cost_weights_are_finite_and_not_negative() {
    let db = Database::new();
    for bad in [f64::NAN, f64::INFINITY, -1.0_f64] {
        assert_eq!(db.with_cost_model(|c| c.sort_compare = bad).map_err(|e| e.code), Err(Code::Api), "{bad}");
        assert_eq!(db.with_cost_model(|c| c.index_descent = bad).map_err(|e| e.code), Err(Code::Api), "{bad}");
    }
    let unchanged = db.with_cost_model(|c| (c.index_descent, c.sort_compare)).unwrap();
    assert_eq!(unchanged, (1.0_f64, 0.1_f64));
    db.with_cost_model(|c| c.sort_compare = 0.0_f64).unwrap();
    assert_eq!(db.with_cost_model(|c| c.sort_compare).unwrap(), 0.0_f64);
}

/// Verifies: [DESIGN] an index method registered through the `Database`
/// replaces the built-in one of that name and serves new indexes.
#[test]
fn index_methods_can_be_registered() {
    let db = Database::default();
    db.register_index_method(Box::new(HashMethod::in_memory())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    db.create_index("T_K", "A.T", &["K"], "hash").unwrap();
    assert_eq!(db.create_index("T_K2", "A.T", &["K"], "nope").map_err(|e| e.code), Err(Code::Api));
}

/// An engine whose scans hold only the columns asked for, every other
/// column NULL, as a column layout's may: if a statement read a column it
/// did not ask for, its result would differ.
struct Sparse;

struct SparseTable(crate::common::MemoryTable);

impl StorageEngine for Sparse {
    fn name(&self) -> &'static str {
        "sparse"
    }
    fn create_table(&self, _: &TableDesc) -> error::Result<Box<dyn TableStorage>> {
        Ok(Box::new(SparseTable(crate::common::MemoryTable::new())))
    }
}

impl TableStorage for SparseTable {
    fn insert(&self, tx: u64, row: Row) -> error::Result<RowId> {
        self.0.insert(tx, row)
    }
    fn update(&self, tx: u64, id: RowId, row: Row) -> error::Result<()> {
        self.0.update(tx, id, row)
    }
    fn delete(&self, tx: u64, id: RowId) -> error::Result<()> {
        self.0.delete(tx, id)
    }
    fn restore(&self, tx: u64, id: RowId, row: Row) -> error::Result<()> {
        self.0.restore(tx, id, row)
    }
    fn get(&self, tx: u64, id: RowId) -> error::Result<Option<Row>> {
        self.0.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: u64, columns: &[usize]) -> Rows {
        let wanted = columns.to_vec();
        Box::new(self.0.live_rows().map(move |read| {
            let (id, row) = read?;
            let sparse = row.into_iter().enumerate().map(|(i, v)| if wanted.contains(&i) { v } else { Value::Null }).collect();
            Ok((id, sparse))
        }))
    }
    fn row_count(&self) -> usize {
        self.0.row_count()
    }
}

/// A database whose tables are in `engine`, with rows in both.
fn sparse_or_not(engine: Option<&str>) -> Database {
    let db = Database::new();
    if let Some(engine) = engine {
        db.register_engine(Box::new(Sparse)).unwrap();
        db.use_engine(engine).unwrap();
    }
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE, A CHAR(3), B INTEGER, C DECIMAL(5,2))
           CREATE TABLE U (K INTEGER, D CHAR(2))",
    )
    .unwrap();
    let s = db.session("A");
    for (k, a, b, c) in [(1, "x", 10, "1.50"), (2, "y", 20, "2.25"), (3, "x", 30, "0.75"), (4, "z", 20, "9.99")] {
        s.run(&format!("INSERT INTO T VALUES ({k}, '{a}', {b}, {c})")).unwrap();
    }
    for (k, d) in [(1, "p"), (3, "q"), (3, "r")] {
        s.run(&format!("INSERT INTO U VALUES ({k}, '{d}')")).unwrap();
    }
    s.commit().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D31, `TableStorage::scan`: every statement
/// asks a scan for each column it reads, so an engine that fills only the
/// columns asked for gives the same results as one that fills them all:
/// select lists, WHERE and ORDER BY on columns not selected, GROUP BY and
/// HAVING, DISTINCT, joins, COUNT(*), `SELECT *`, and reads after UPDATE
/// and DELETE.
#[test]
fn scans_read_the_columns_statements_use() {
    let queries = [
        "SELECT K FROM T WHERE B = 20 ORDER BY K",
        "SELECT A FROM T WHERE C > 1 ORDER BY B DESC, K",
        "SELECT A, SUM(B), MAX(C) FROM T GROUP BY A HAVING COUNT(*) > 1",
        "SELECT DISTINCT A FROM T ORDER BY A",
        "SELECT T.K, U.D FROM T, U WHERE T.K = U.K ORDER BY 2",
        "SELECT COUNT(*) FROM T",
        "SELECT * FROM T ORDER BY K",
        "SELECT K + B FROM T WHERE A = 'x' ORDER BY 1",
    ];
    let full = sparse_or_not(None);
    let sparse = sparse_or_not(Some("sparse"));
    for db in [&full, &sparse] {
        let s = db.session("A");
        s.run("UPDATE T SET C = C * 2 WHERE A = 'x'").unwrap();
        s.run("DELETE FROM U WHERE D = 'r'").unwrap();
        s.commit().unwrap();
    }
    for q in queries {
        assert_eq!(crate::common::rows(&sparse.session("A"), q), crate::common::rows(&full.session("A"), q), "{q}");
    }
}

/// A catalog store kept apart from the directory, as a replicated or remote
/// one would be: its records in memory, shared by every store it makes or
/// opens, the one place it keeps.
#[derive(Clone, Default)]
pub struct HeldCatalogs(pub Arc<std::sync::Mutex<Option<Vec<Vec<u8>>>>>);

struct HeldCatalog(Arc<std::sync::Mutex<Option<Vec<Vec<u8>>>>>);

impl CatalogStorage for HeldCatalogs {
    fn name(&self) -> &'static str {
        "held"
    }
    fn open(&self, _: &StorePlace<'_>) -> error::Result<Option<Box<dyn CatalogStore>>> {
        Ok(self.0.lock().unwrap().is_some().then(|| Box::new(HeldCatalog(Arc::clone(&self.0))) as Box<dyn CatalogStore>))
    }
    fn create(&self, _: &StorePlace<'_>) -> error::Result<Box<dyn CatalogStore>> {
        let mut records = self.0.lock().unwrap();
        if records.is_some() {
            return error::fail(Code::Io, "a catalog is there");
        }
        *records = Some(Vec::new());
        Ok(Box::new(HeldCatalog(Arc::clone(&self.0))))
    }
}

impl CatalogStore for HeldCatalog {
    fn record(&self, record: &[u8], compact_after: usize) -> error::Result<()> {
        let mut records = self.0.lock().unwrap();
        let records = records.as_mut().unwrap();
        if records.len() >= compact_after {
            records.clear();
        }
        records.push(record.to_vec());
        Ok(())
    }
    fn last(&self) -> Option<Vec<u8>> {
        self.0.lock().unwrap().as_ref()?.last().cloned()
    }
}

/// The contract of `CatalogStorage` and `CatalogStore`, for `storage` at
/// `dir`: none where none was made; one made, holding no record; a second
/// refused; each record the last until the next, kept past `compact_after`
/// of them, and given by the store opened again.
fn catalog_storage_contract(storage: &dyn CatalogStorage, dir: &std::path::Path) {
    let (fs, hash) = (crate::common::os(), crc32c());
    std::fs::create_dir_all(dir).unwrap();
    let formats = FileFormats::of(hash);
    let place = StorePlace::new(&fs, dir, &formats);
    assert!(storage.open(&place).unwrap().is_none(), "{}: none made", storage.name());
    let store = storage.create(&place).unwrap();
    assert!(storage.create(&place).is_err(), "{}: one is there", storage.name());
    assert_eq!(store.last(), None);
    for n in 0_u32..10_u32 {
        store.record(&n.to_le_bytes(), 4).unwrap();
        assert_eq!(store.last(), Some(n.to_le_bytes().to_vec()), "{}", storage.name());
    }
    drop(store);
    assert_eq!(storage.open(&place).unwrap().unwrap().last(), Some(9_u32.to_le_bytes().to_vec()), "{}: opened again", storage.name());
}

/// Verifies: [EXTENSION] RULES.md D25, R-SAFE-3: the built-in catalog
/// store, and one kept apart from the directory, keep the catalog store
/// contract (`db/stores.rs`).
#[test]
fn catalog_stores_keep_the_catalog_store_contract() {
    let scratch = Scratch::new("contract-catalog");
    catalog_storage_contract(&CatalogFiles, &scratch.path("files"));
    catalog_storage_contract(&HeldCatalogs::default(), &scratch.path("held"));
}

/// A log kept apart from the directory, as a replicated or remote one
/// would be: a memory log, shared by every storage it is opened through.
#[derive(Clone, Default)]
pub struct HeldLogs(pub Arc<std::sync::Mutex<Option<Arc<MemoryLog>>>>);

impl LogStorage for HeldLogs {
    fn name(&self) -> &'static str {
        "held"
    }
    fn open(&self, _: &StorePlace<'_>) -> error::Result<Option<Arc<dyn Log>>> {
        Ok(self.0.lock().unwrap().as_ref().map(|log| Arc::clone(log) as Arc<dyn Log>))
    }
    fn exists(&self, _: &StorePlace<'_>) -> error::Result<bool> {
        Ok(self.0.lock().unwrap().is_some())
    }
    fn create(&self, _: &StorePlace<'_>) -> error::Result<Arc<dyn Log>> {
        let mut held = self.0.lock().unwrap();
        if held.is_some() {
            return error::fail(Code::Api, "a log is there");
        }
        let log = Arc::new(MemoryLog::new());
        *held = Some(Arc::clone(&log));
        Ok(log)
    }
}

/// The contract of `LogStorage`, for `storage` at `dir`: none where none
/// was made, nor said to exist; one made, holding no record, said to; a second refused; the records
/// appended and synced given back, in order, by the log opened again.
fn log_storage_contract(storage: &dyn LogStorage, dir: &std::path::Path) {
    let (fs, hash) = (crate::common::os(), crc32c());
    std::fs::create_dir_all(dir).unwrap();
    let formats = FileFormats::of(hash);
    let place = StorePlace::new(&fs, dir, &formats);
    assert!(storage.open(&place).unwrap().is_none(), "{}: none made", storage.name());
    assert!(!storage.exists(&place).unwrap(), "{}: none there", storage.name());
    let log = storage.create(&place).unwrap();
    assert!(storage.exists(&place).unwrap(), "{}: one there", storage.name());
    assert!(storage.create(&place).is_err(), "{}: one is there", storage.name());
    assert_eq!(log.read_from(log.end()).count(), 0);
    let start = log.end();
    let mut last = start;
    for n in 0_u32..5_u32 {
        last = log.append(&n.to_le_bytes()).unwrap();
    }
    log.sync(last).unwrap();
    drop(log);
    let again = storage.open(&place).unwrap().unwrap();
    let read: Vec<Vec<u8>> = again.read_from(start).map(|r| r.unwrap().1).collect();
    assert_eq!(read, (0_u32..5_u32).map(|n| n.to_le_bytes().to_vec()).collect::<Vec<_>>(), "{}: opened again", storage.name());
}

/// Verifies: [EXTENSION] RULES.md D25, R-SAFE-3: the built-in log
/// storage, and one kept apart from the directory, keep the log storage
/// contract (`db/stores.rs`).
#[test]
fn log_storages_keep_the_log_storage_contract() {
    let scratch = Scratch::new("contract-logs");
    log_storage_contract(&FileLogs::default(), &scratch.path("files"));
    log_storage_contract(&HeldLogs::default(), &scratch.path("held"));
}
