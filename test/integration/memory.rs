//! The memory budget (RULES.md D38): a database's limit on the bytes its
//! buffer pools, and its plug-ins' caches, reserve, the setting
//! MEMORY.BUDGET_BYTES's.

use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::{Database, OpenOptions};
use turborel::engine::rows::{RowStore, TableOptions};
use turborel::error::{Code, Result, SqlError};
use turborel::memory::{Host, Reservation};
use turborel::storage::{StorageEngine, TableStorage};
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, os};

const MIB: usize = 1 << 20;
const HALF_MIB: usize = 1 << 19;
/// The page that heads a pager file: what a file in memory holds once made.
const HEADER: usize = 8192;

fn bytes(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0))
}

/// Verifies: [DESIGN] RULES.md D38, D56: a database in memory reserves the
/// pool its engine and index methods share as its first table is made, of
/// STORAGE.POOL_BYTES then, 32 MiB unless set, of MEMORY.BUDGET_BYTES's 256 MiB,
/// with the page heading their file, its indexes taking no more; past the budget a statement is refused
/// (SQLCODE -918, SQLSTATE TB918); STORAGE.POOL_BYTES set after the pool is
/// made changes nothing; a lower budget takes back nothing reserved; one
/// below 1 MiB is no MEMORY.BUDGET_BYTES.
#[test]
fn pools_reserve_the_budget() {
    let db = Database::new();
    assert_eq!(db.memory().unwrap(), (256 * MIB, 0));
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let (limit, made) = db.memory().unwrap();
    assert!(limit == 256 * MIB && (32 * MIB + HEADER..33 * MIB).contains(&made), "the shared pool and its file: {made}");
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (K INTEGER UNIQUE)").unwrap();
    assert!((made..33 * MIB).contains(&db.memory().unwrap().1), "its index method shares them");

    let small = Database::new();
    small.set_setting("MEMORY.BUDGET_BYTES", &bytes(3 * MIB)).unwrap();
    small.set_setting("STORAGE.POOL_BYTES", &bytes(4 * MIB)).unwrap();
    let refused = small.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap_err();
    assert_eq!((refused.code, refused.sqlstate().as_str()), (Code::Resources, "TB918"), "{}", refused.msg);
    small.set_setting("STORAGE.POOL_BYTES", &bytes(2 * MIB)).unwrap();
    small.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    small.set_setting("STORAGE.POOL_BYTES", &bytes(MIB)).unwrap();
    small.set_setting("MEMORY.BUDGET_BYTES", &bytes(MIB)).unwrap();
    let (limit, held) = small.memory().unwrap();
    assert!(limit == MIB && (2 * MIB + HEADER..3 * MIB).contains(&held), "nothing taken back, the pool as it was made");
    small.reset_setting("MEMORY.BUDGET_BYTES").unwrap();
    small.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (K INTEGER UNIQUE)").unwrap();
    assert!((held..3 * MIB).contains(&small.memory().unwrap().1));
    assert_eq!(small.set_setting("MEMORY.BUDGET_BYTES", &bytes(MIB - 1)).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D38: a database kept in a directory makes
/// its buffer pool of STORAGE.POOL_BYTES, given as it opens, in the budget it recorded:
/// opened again, a pool larger than the budget set is refused (SQLCODE
/// -918), a smaller one is made in it.
#[test]
fn a_kept_database_makes_its_pool_in_its_budget() {
    let scratch = Scratch::new("memory-budget");
    let dir = scratch.path("db");
    let open = |pool_bytes: usize| {
        let db = Database::new();
        db.set_setting("STORAGE.POOL_BYTES", &bytes(pool_bytes)).unwrap();
        db.open_files(&os(), &dir, &OpenOptions::default()).map(|_| db)
    };
    let db = open(64 * 8192).unwrap();
    assert_eq!(db.memory().unwrap(), (256 * MIB, 64 * 8192));
    db.set_setting("MEMORY.BUDGET_BYTES", &bytes(MIB)).unwrap();
    db.close().unwrap();
    drop(db);
    assert_eq!(open(2 * MIB).err().map(|e| e.code), Some(Code::Resources), "past the budget recorded");
    let db = open(HALF_MIB).unwrap();
    assert_eq!(db.memory().unwrap(), (MIB, HALF_MIB));
    assert_eq!(db.setting("MEMORY.BUDGET_BYTES").unwrap().to_literal(), MIB.to_string());
}

/// A plug-in engine with a cache of its own: a megabyte reserved of the
/// budget it is given for each table it makes, its tables kept by a row
/// store it gives the budget too; or one that refuses any budget.
struct Cached {
    rows: RowStore,
    host: Mutex<Option<Host>>,
    caches: Mutex<Vec<Reservation>>,
    refuses: bool,
}

impl Cached {
    fn new(refuses: bool) -> Cached {
        Cached { rows: RowStore::in_memory(TableOptions::default()), host: Mutex::new(None), caches: Mutex::new(Vec::new()), refuses }
    }
}

impl StorageEngine for Cached {
    fn name(&self) -> &'static str {
        "cached"
    }
    fn attach(&self, host: &Host) -> Result<()> {
        if self.refuses {
            return Err(SqlError::new(Code::Api, "no budget for me"));
        }
        self.rows.attach(host)?;
        *self.host.lock().unwrap() = Some(host.clone());
        Ok(())
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        let memory = Arc::clone(&self.host.lock().unwrap().as_ref().unwrap().memory);
        self.caches.lock().unwrap().push(memory.reserve(MIB, "a cache")?);
        self.rows.create_table(desc)
    }
}

/// Verifies: [DESIGN] RULES.md D38, D30, `StorageEngine::attach`:
/// a plug-in is given the database's memory budget as it is registered,
/// so its caches, and the pools of the engines it is made of, draw on the
/// same limit as the built-in ones, refused past it (SQLCODE -918); one
/// that refuses what it is given is not registered.
#[test]
fn plug_ins_draw_on_the_budget() {
    let db = Database::new();
    db.register_engine(Box::new(Cached::new(false))).unwrap();
    db.set_setting("TABLES.ENGINE", &Value::Char("cached".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE TABLE U (K INTEGER)").unwrap();
    let held = db.memory().unwrap().1;
    assert!((4 * MIB + HEADER..5 * MIB).contains(&held), "its row store's pool and file, and a cache for each table: {held}");
    db.set_setting("MEMORY.BUDGET_BYTES", &bytes(4 * MIB)).unwrap();
    let refused = db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE V (K INTEGER)").unwrap_err();
    assert_eq!(refused.code, Code::Resources, "{}", refused.msg);
    assert_eq!(db.register_engine(Box::new(Cached::new(true))).err().map(|e| e.code), Some(Code::Api));
    db.session("A").run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(crate::common::count(&db.session("A"), "SELECT K FROM T"), 1, "the first, still registered");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md D38:
/// INFORMATION_SCHEMA.MEMORY_RESERVATIONS shows the database's owner what
/// holds each reservation of the budget, and its bytes, as many in all as
/// the budget has reserved; anyone else sees none.
#[test]
fn reservations_are_shown_to_the_owner() {
    let scratch = Scratch::new("memory-reservations");
    let db = Database::new();
    db.set_setting("STORAGE.POOL_BYTES", &bytes(HALF_MIB)).unwrap();
    db.open_files(&os(), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let shown = "SELECT TRIM(RESERVED_BY), BYTES FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS ORDER BY 1";
    let owner = db.session("OWNER");
    let held = crate::common::rows(&owner, shown).unwrap();
    assert!(held.contains(&format!("the buffer pool of the data file,{HALF_MIB}")), "{held:?}");
    let total: usize = held.iter().map(|r| r.rsplit(',').next().unwrap().parse::<usize>().unwrap()).sum();
    assert_eq!(total, db.memory().unwrap().1);
    assert_eq!(crate::common::rows(&db.session("OTHER"), shown), Ok(vec![]));
}

/// Verifies: [DESIGN] RULES.md D38, D56: the pages of a database in memory
/// are reserved of its budget as its file grows past the pool, so a table
/// that outgrows MEMORY.BUDGET_BYTES is refused (SQLCODE -918), not given memory
/// past it; the rows written before stay.
#[test]
fn a_database_in_memory_keeps_its_pages_in_its_budget() {
    let db = Database::new();
    db.set_setting("MEMORY.BUDGET_BYTES", &bytes(3 * MIB)).unwrap();
    db.set_setting("STORAGE.POOL_BYTES", &bytes(MIB)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, S CHARACTER(1000))").unwrap();
    let a = db.session("A");
    let long = "x".repeat(1000);
    let refused = (0_i32..10_000_i32).find_map(|k| a.run(&format!("INSERT INTO T VALUES ({k}, '{long}')")).err().map(|e| (k, e)));
    let (written, error) = refused.expect("a table past the budget is refused");
    assert_eq!(error.code, Code::Resources, "{}", error.msg);
    assert!(written > 1000_i32, "{written} rows first");
    assert!(db.memory().unwrap().1 <= 3 * MIB);
    assert_eq!(crate::common::count(&a, "SELECT K FROM T"), usize::try_from(written).unwrap());
}

/// Verifies: [DESIGN] RULES.md D56: a database in memory spills to files in
/// memory, reserved of its budget while they are held and given back after,
/// never to the system's temporary directory.
#[test]
fn a_database_in_memory_spills_in_memory() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, S CHARACTER(200))").unwrap();
    let a = db.session("A");
    for first in (0_i32..4000_i32).step_by(500) {
        let values: Vec<String> = (first..first + 500_i32).map(|k| format!("({k}, 'r{k}')")).collect();
        a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    let before = db.memory().unwrap().1;
    a.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    let sorted = crate::common::rows(&a, "SELECT K, S FROM T ORDER BY S DESC").unwrap();
    let sorted: Vec<String> = sorted.iter().map(|r| r.trim_end().to_owned()).collect();
    assert_eq!((sorted.len(), sorted.first().map(String::as_str)), (4000, Some("999,r999")));
    assert_eq!(db.memory().unwrap().1, before, "given back");
    let ours = format!("turborel-{}-", std::process::id());
    let left: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(&ours))
        .collect();
    assert_eq!(left, Vec::<String>::new(), "nothing of this process in the temporary directory");
}
