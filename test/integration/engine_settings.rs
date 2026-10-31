//! Settings a storage engine declares and reads (RULES.md D36, D41):
//! declared under its name as it is registered, their values told it as
//! they are published (`StorageEngine::configure`), with no lock held;
//! the column store's bound on the headers of frozen groups among them.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use turborel::catalog::TableDesc;
use turborel::db::{Database, OpenOptions};
use turborel::error::Result;
use turborel::settings::{Definition, Effect, Kind, Settings};
use turborel::storage::{StorageEngine, TableStorage};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{MemoryTable, Scratch, after_crash, rows};

fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// An engine of tables in memory that declares NOTED.DEPTH, or a setting
/// of another's name, and keeps the value it is told; -1 until told.
struct Noted {
    setting: &'static str,
    told: Arc<AtomicI64>,
}

impl Noted {
    fn new(setting: &'static str) -> (Noted, Arc<AtomicI64>) {
        let told = Arc::new(AtomicI64::new(-1));
        (Noted { setting, told: Arc::clone(&told) }, told)
    }
}

impl StorageEngine for Noted {
    fn name(&self) -> &'static str {
        "noted"
    }
    fn settings(&self) -> Vec<Definition> {
        vec![Definition {
            name: self.setting.to_owned(),
            kind: Kind::Integer { min: 1, max: 9 },
            default: whole(1),
            per_session: false,
            effect: Effect::Statement,
            doc: "How deep.".to_owned(),
        }]
    }
    fn configure(&self, settings: &Settings) {
        if let Some(key) = settings.key(self.setting) {
            self.told.store(i64::try_from(settings.integer(key)).unwrap(), Ordering::SeqCst);
        }
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(MemoryTable::new()))
    }
}

/// Verifies: [DESIGN] RULES.md D36, R-SAFE-3: an engine's setting is
/// declared as it is registered and its value told it: its default at
/// once, each value the database is given after, its default again once
/// reset; kept with the definitions, so an engine registered again as the
/// database is opened again is told the value given before; a setting
/// not under the engine's name is refused, the engine not registered.
#[test]
fn an_engine_reads_its_settings_as_they_are_published() {
    let scratch = Scratch::new("engine-settings");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    let (engine, told) = Noted::new("NOTED.DEPTH");
    db.register_engine(Box::new(engine)).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 1, "its default");
    db.set_setting("noted.depth", &whole(4)).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 4);
    db.reset_setting("NOTED.DEPTH").unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 1, "its default again");
    db.set_setting("NOTED.DEPTH", &whole(6)).unwrap();
    db.close().unwrap();
    drop(db);
    let again = Database::new();
    again.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    let (engine, told) = Noted::new("NOTED.DEPTH");
    again.register_engine(Box::new(engine)).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 6, "kept");
    again.set_setting("NOTED.DEPTH", &whole(7)).unwrap();
    let crashed = again;
    let third = Database::new();
    let (engine, told) = Noted::new("NOTED.DEPTH");
    third.register_engine(Box::new(engine)).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 1, "its default, until opened");
    drop(crashed);
    third.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 7, "recovered, as it opens");
    let other = Database::new();
    let (stray, untold) = Noted::new("OTHER.DEPTH");
    assert_eq!(other.register_engine(Box::new(stray)).map_err(|e| e.sqlcode()), Err(-910_i32));
    assert_eq!(untold.load(Ordering::SeqCst), -1, "not told");
    assert!(other.setting("OTHER.DEPTH").is_err() && other.place_table("X.T", "noted").is_err(), "not registered");
}

/// The bytes of the budget the headers of frozen row groups hold, as
/// INFORMATION_SCHEMA.MEMORY_RESERVATIONS shows the database's owner, OWN.
fn header_bytes(db: &Database) -> String {
    let sql = "SELECT COALESCE(SUM(BYTES), 0) FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS WHERE RESERVED_BY LIKE '%headers of frozen%'";
    rows(&db.session("OWN"), sql).unwrap().remove(0)
}

/// Verifies: [DESIGN] RULES.md D31, D36, D38: the column store's
/// COLUMNSTORE.HEADER_CACHE_BYTES, of 16 MiB by default, bounds the
/// headers of frozen groups it holds: set to 0 in SQL by the owner, it
/// gives up those held, their memory given back, and holds none read
/// after; given its default again, it holds them again.
///
/// Regression test (R-TEST-7): the column store of a database kept in
/// memory held its headers of no budget, so none were reserved.
#[test]
fn the_column_store_holds_headers_as_its_setting_says() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.create_user("OWN", b"").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE C (K INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..4096_i32).map(|k| format!("({k})")).collect();
    a.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    let sum = || rows(&a, "SELECT SUM(K) FROM C").unwrap();
    let total = vec!["8386560".to_owned()];
    assert_eq!(db.setting("COLUMNSTORE.HEADER_CACHE_BYTES").unwrap().to_string(), "16777216");
    assert_eq!(sum(), total);
    assert_ne!(header_bytes(&db), "0", "held");
    let own = db.session("OWN");
    own.run("SET DATABASE COLUMNSTORE.HEADER_CACHE_BYTES = 0").unwrap();
    assert_eq!(header_bytes(&db), "0", "given up");
    assert_eq!(sum(), total);
    assert_eq!(header_bytes(&db), "0", "none held");
    own.run("SET DATABASE COLUMNSTORE.HEADER_CACHE_BYTES TO DEFAULT").unwrap();
    assert_eq!(sum(), total);
    assert_ne!(header_bytes(&db), "0", "held again");
}
