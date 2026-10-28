//! Table settings (RULES.md D41): the options a storage engine declares for
//! its tables, set by `CREATE TABLE ... USING engine WITH (...)`, changed by
//! `ALTER TABLE ... SET (...)` and `ALTER TABLE ... USING engine`, in place
//! or by a rebuild, and listed in INFORMATION_SCHEMA.TABLE_OPTIONS. The
//! plug-ins here serve `index_settings.rs` too.

use std::sync::{Arc, Mutex};

use turborel::catalog::{IndexDesc, TableDesc};
use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, Result};
use turborel::index::{BTreeMethod, Index, IndexMethod, Key, Probe};
use turborel::settings::Kind;
use turborel::storage::{Rebuild, Row, RowId, Rows, StorageEngine, TableOption, TableStorage};
use turborel::txn::TxId;
use turborel::value::{CompOp, DataType, Decimal, Value};

use crate::common::{MemoryTable, Scratch, exec, os, rows};

/// What the engine was asked: each table made, and each told its options
/// changed, with the options its descriptor sets.
#[derive(Clone, Default)]
pub(crate) struct Calls(Arc<Mutex<Vec<String>>>);

impl Calls {
    pub(crate) fn note(&self, what: &str, desc: &TableDesc) {
        self.note_index(what, (desc, &[]));
    }

    /// As `note`, then the index's options, if it sets any.
    fn note_index(&self, what: &str, (desc, index): (&TableDesc, &[(String, Value)])) {
        let shown = |options: &[(String, Value)]| options.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join(",");
        let mut line = format!("{what} {} {}", desc.name.ident, shown(&desc.options));
        if !index.is_empty() {
            line = format!("{line} / {}", shown(index));
        }
        self.0.lock().unwrap().push(line);
    }

    /// The calls since the last.
    pub(crate) fn taken(&self) -> Vec<String> {
        self.0.lock().unwrap().drain(..).collect()
    }
}

/// An engine of `MemoryTable`s, "tuned", whose tables take PACE, changed
/// in place, and LAYOUT, changed by a rebuild.
struct Tuned(Calls);

impl StorageEngine for Tuned {
    fn name(&self) -> &'static str {
        "tuned"
    }
    fn table_options(&self) -> Vec<TableOption> {
        vec![
            TableOption::new(("pace", Kind::Integer { min: 1, max: 100 }, whole(10)), Rebuild::No, "How fast."),
            TableOption::new(("LAYOUT", Kind::Choice(&["ROWS", "COLUMNS"]), Value::Char("ROWS".into())), Rebuild::Yes, "How kept."),
        ]
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        self.0.note("create", desc);
        Ok(Box::new(TunedTable { rows: Arc::new(MemoryTable::new()), calls: self.0.clone() }))
    }
}

/// A table of `Tuned`.
struct TunedTable {
    rows: Arc<MemoryTable>,
    calls: Calls,
}

impl TableStorage for TunedTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.rows.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.rows.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.rows.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        self.rows.live_rows()
    }
    fn row_count(&self) -> usize {
        self.rows.row_count()
    }
    fn reconfigure(&self, desc: &TableDesc) {
        self.calls.note("reconfigure", desc);
    }
}

pub(crate) fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A database with `Tuned` registered, and what it is asked.
fn database() -> (Database, Calls) {
    let db = Database::new();
    let calls = Calls::default();
    db.register_engine(Box::new(Tuned(calls.clone()))).unwrap();
    (db, calls)
}

/// The options of table `table` of schema S, as TABLE_OPTIONS lists them:
/// engine, name, value and source.
fn options(db: &Database, table: &str) -> Vec<String> {
    let sql = format!(
        "SELECT ENGINE_NAME, OPTION_NAME, OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.TABLE_OPTIONS \
         WHERE TABLE_SCHEMA = 'S' AND TABLE_NAME = '{table}' ORDER BY OPTION_NAME"
    );
    rows(&db.session("S"), &sql).unwrap().into_iter().map(|r| r.replace(' ', "")).collect()
}

/// The options of table `table` of schema S, with who declared each:
/// engine, name, declarer, value and source.
fn declared(db: &Database, table: &str) -> Vec<String> {
    let sql = format!(
        "SELECT ENGINE_NAME, OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.TABLE_OPTIONS \
         WHERE TABLE_SCHEMA = 'S' AND TABLE_NAME = '{table}' ORDER BY OPTION_NAME"
    );
    rows(&db.session("S"), &sql).unwrap().into_iter().map(|r| r.replace(' ', "")).collect()
}

/// Verifies: [EXTENSION] RULES.md D41: CREATE TABLE ... USING an engine,
/// in any case, WITH options of its, each taken of its kind, gives the
/// table to that engine with them, and TABLE_OPTIONS lists every option of
/// it, with its value and whether the table set it; an engine not
/// registered, an option it does not declare, one given twice or a value
/// not of its kind is a Syntax Rule broken (SQLCODE -103), and the
/// `<schema>` makes nothing.
#[test]
fn a_table_is_created_in_its_engine_with_its_options() {
    let (db, calls) = database();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER) USING Tuned WITH (PACE = 20, layout = columns)").unwrap();
    assert_eq!(calls.taken(), ["create T LAYOUT=COLUMNS,PACE=20"]);
    assert_eq!(options(&db, "T"), ["tuned,ENCODINGS,,DEFAULT", "tuned,LAYOUT,COLUMNS,TABLE", "tuned,PACE,20,TABLE"]);
    db.schema("CREATE SCHEMA AUTHORIZATION R CREATE TABLE U (K INTEGER) USING TUNED").unwrap();
    let refused = [
        "CREATE TABLE V (K INTEGER) USING NOPE",
        "CREATE TABLE V (K INTEGER) USING TUNED WITH (SPEED = 1)",
        "CREATE TABLE V (K INTEGER) USING TUNED WITH (PACE = 1, PACE = 2)",
        "CREATE TABLE V (K INTEGER) USING TUNED WITH (PACE = 'FAST')",
        "CREATE TABLE V (K INTEGER) USING TUNED WITH (PACE = 1000)",
        "CREATE TABLE V (K INTEGER) USING TUNED WITH (LAYOUT = TREES)",
    ];
    for sql in refused {
        let e = db.schema(&format!("CREATE SCHEMA AUTHORIZATION Q {sql}")).unwrap_err();
        assert_eq!(e.code, Code::SyntaxRule, "{sql}: {}", e.msg);
    }
    assert_eq!(calls.taken(), ["create U "], "nothing of the refused");
    assert_eq!(rows(&db.session("Q"), "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'Q'"), Ok(vec!["0".into()]));
}

/// Verifies: [EXTENSION] RULES.md D41: ALTER TABLE ... SET changes an
/// option that changes in place by telling the table's storage once the
/// change is published, at commit, and not if it is rolled back; one that
/// needs a rebuild makes new storage with it, the rows copied there; an
/// option set to DEFAULT is the table's no more; the table's rows and
/// constraints are as they were.
#[test]
fn options_change_in_place_or_by_a_rebuild() {
    let (db, calls) = database();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(4)) USING TUNED").unwrap();
    let s = db.session("S");
    for k in 0..5 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{k}')")).unwrap();
    }
    s.commit().unwrap();
    calls.taken();
    exec(&s, "ALTER TABLE T SET (PACE = 50)").unwrap();
    assert_eq!(calls.taken(), Vec::<String>::new(), "not before it commits");
    s.rollback().unwrap();
    assert_eq!(calls.taken(), Vec::<String>::new(), "nor when it is rolled back");
    assert_eq!(options(&db, "T"), ["tuned,ENCODINGS,,DEFAULT", "tuned,LAYOUT,ROWS,DEFAULT", "tuned,PACE,10,DEFAULT"]);
    exec(&s, "ALTER TABLE T SET (PACE = 50)").unwrap();
    s.commit().unwrap();
    assert_eq!(calls.taken(), ["reconfigure T PACE=50"]);
    exec(&s, "ALTER TABLE T SET (LAYOUT = COLUMNS, PACE = DEFAULT)").unwrap();
    s.commit().unwrap();
    assert_eq!(calls.taken(), ["create T LAYOUT=COLUMNS"], "a rebuild");
    assert_eq!(options(&db, "T"), ["tuned,ENCODINGS,,DEFAULT", "tuned,LAYOUT,COLUMNS,TABLE", "tuned,PACE,10,DEFAULT"]);
    assert_eq!(rows(&s, "SELECT K, V FROM T ORDER BY K").unwrap().len(), 5);
    assert_eq!(exec(&s, "INSERT INTO T VALUES (1, 'x')"), Err(Code::Constraint), "its UNIQUE constraint holds");
    s.rollback().unwrap();
    assert_eq!(exec(&s, "ALTER TABLE T SET (SPEED = 1)"), Err(Code::SyntaxRule));
    s.rollback().unwrap();
    assert_eq!(exec(&db.session("R"), "ALTER TABLE S.T SET (PACE = 2)"), Err(Code::Privilege), "only its owner alters it");
}

/// Verifies: [EXTENSION] RULES.md D41: ALTER TABLE ... USING moves a table
/// to another engine, with the options given and no other, its rows and
/// constraints with it; the row store's own options are listed, and a
/// change of its statistics' sampling is made in place.
#[test]
fn a_table_moves_between_engines() {
    let (db, calls) = database();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE) USING TUNED WITH (PACE = 3)").unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T VALUES (1)").unwrap();
    s.run("INSERT INTO T VALUES (2)").unwrap();
    s.commit().unwrap();
    exec(&s, "ALTER TABLE T USING ROWSTORE WITH (STATISTICS.SAMPLE = 100)").unwrap();
    s.commit().unwrap();
    let listed = options(&db, "T");
    assert!(listed.contains(&"rowstore,STATISTICS.SAMPLE,100,TABLE".to_owned()), "{listed:?}");
    assert!(listed.contains(&"rowstore,OUT_OF_LINE,0,DEFAULT".to_owned()), "{listed:?}");
    assert_eq!(rows(&s, "SELECT K FROM T ORDER BY K"), Ok(vec!["1".into(), "2".into()]));
    assert_eq!(exec(&s, "INSERT INTO T VALUES (2)"), Err(Code::Constraint));
    s.rollback().unwrap();
    exec(&s, "ALTER TABLE T SET (STATISTICS.BUCKETS = 10, OUT_OF_LINE = 8)").unwrap();
    s.commit().unwrap();
    exec(&s, "ALTER TABLE T USING TUNED").unwrap();
    s.commit().unwrap();
    calls.taken();
    assert_eq!(
        options(&db, "T"),
        ["tuned,ENCODINGS,,DEFAULT", "tuned,LAYOUT,ROWS,DEFAULT", "tuned,PACE,10,DEFAULT"],
        "none of the row store's"
    );
    assert_eq!(rows(&s, "SELECT K FROM T ORDER BY K"), Ok(vec!["1".into(), "2".into()]));
}

/// Verifies: [EXTENSION] RULES.md D41, D25: a table's options are part of
/// its definition: a database kept in a directory has them, as set and as
/// changed, once opened again.
#[test]
fn options_survive_the_database_closing() {
    let scratch = Scratch::new("table-settings");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER) WITH (STATISTICS.SAMPLE = 200, OUT_OF_LINE = 16)").unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T VALUES (7)").unwrap();
    exec(&s, "ALTER TABLE T SET (STATISTICS.SAMPLE = 300)").unwrap();
    s.commit().unwrap();
    drop(s);
    let before = options(&db, "T");
    db.close().unwrap();
    drop(db);
    let db = open();
    assert_eq!(options(&db, "T"), before);
    assert!(before.contains(&"rowstore,STATISTICS.SAMPLE,300,TABLE".to_owned()), "{before:?}");
    assert_eq!(rows(&db.session("S"), "SELECT K FROM T"), Ok(vec!["7".into()]));
}

/// An index method, "noted", whose indexes are the B-link tree's, that
/// notes the table it makes or is told of, and declares two options of
/// every table: NOTED.FILL, changed in place, and NOTED.SHAPE, by a rebuild.
pub(crate) struct Noted(pub(crate) BTreeMethod, pub(crate) Calls);

impl IndexMethod for Noted {
    fn name(&self) -> &'static str {
        "noted"
    }
    fn supports(&self, op: CompOp) -> bool {
        self.0.supports(op)
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        self.0.create(columns)
    }
    fn index_options(&self) -> Vec<TableOption> {
        vec![
            TableOption::new(("FILL", Kind::Integer { min: 1, max: 100 }, whole(90)), Rebuild::No, "How full its nodes are kept."),
            TableOption::new(("SHAPE", Kind::Choice(&["WIDE", "TALL"]), Value::Char("WIDE".into())), Rebuild::Yes, "How it is shaped."),
        ]
    }
    fn create_on(&self, columns: &[DataType], (table, index): (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        self.1.note_index("index", (table, &index.options));
        Ok(Box::new(NotedIndex(self.0.create(columns)?, self.1.clone())))
    }
}

/// An index of `Noted`.
struct NotedIndex(Box<dyn Index>, Calls);

impl Index for NotedIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.0.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.0.remove(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.0.rows(key)
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.0.estimate(probe)
    }
    fn search(&self, probe: &Probe) -> Result<Vec<(Key, RowId)>> {
        self.0.search(probe)
    }
    fn reconfigure(&self, (table, index): (&TableDesc, &IndexDesc)) {
        self.1.note_index("index told", (table, &index.options));
    }
}

/// The options "noted" declares.
fn noted_options() -> [TableOption; 2] {
    [
        TableOption::new(("NOTED.FILL", Kind::Integer { min: 1, max: 100 }, whole(90)), Rebuild::No, "How full."),
        TableOption::new(("NOTED.SHAPE", Kind::Choice(&["WIDE", "TALL"]), Value::Char("WIDE".into())), Rebuild::Yes, "How shaped."),
    ]
}

/// `database`, with "noted" registered, its options declared, behind
/// UNIQUE constraints.
pub(crate) fn noted() -> (Database, Calls) {
    let (db, calls) = database();
    db.register_index_method(Box::new(Noted(BTreeMethod::in_memory(), calls.clone()))).unwrap();
    for option in noted_options() {
        db.declare_table_option("noted", option).unwrap();
    }
    db.set_setting("UNIQUE_INDEX_METHOD", &Value::Char("noted".into())).unwrap();
    (db, calls)
}

/// Verifies: [EXTENSION] RULES.md D41, `Database::declare_table_option`: a
/// plug-in, here an index method, declares options of every table, of any
/// engine, set and changed as an engine's are: its indexes are made with
/// their table's (`IndexMethod::create_on`), told of a change made in
/// place, and made anew by a rebuild; a table moved to another engine keeps
/// them. An option is declared once, by an engine or by a plug-in.
#[test]
fn plug_ins_declare_options_of_every_table() {
    let (db, calls) = noted();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE) WITH (NOTED.FILL = 70)").unwrap();
    assert_eq!(calls.taken(), ["index T NOTED.FILL=70"]);
    assert!(declared(&db, "T").contains(&"rowstore,NOTED.FILL,noted,70,TABLE".to_owned()), "{:?}", declared(&db, "T"));
    let s = db.session("S");
    s.run("INSERT INTO T VALUES (1)").unwrap();
    exec(&s, "ALTER TABLE T SET (NOTED.FILL = 80)").unwrap();
    s.commit().unwrap();
    assert_eq!(calls.taken(), ["index told T NOTED.FILL=80"]);
    exec(&s, "ALTER TABLE T SET (NOTED.SHAPE = TALL)").unwrap();
    s.commit().unwrap();
    assert_eq!(calls.taken(), ["index T NOTED.FILL=80,NOTED.SHAPE=TALL"], "a rebuild makes the index anew");
    exec(&s, "ALTER TABLE T USING TUNED WITH (PACE = 5)").unwrap();
    s.commit().unwrap();
    let listed = declared(&db, "T");
    assert!(
        listed.contains(&"tuned,NOTED.SHAPE,noted,TALL,TABLE".to_owned()) && listed.contains(&"tuned,PACE,tuned,5,TABLE".to_owned()),
        "{listed:?}"
    );
    assert_eq!(exec(&s, "INSERT INTO T VALUES (1)"), Err(Code::Constraint), "its UNIQUE constraint, of noted's index");
    s.rollback().unwrap();
    let declared = |name: &str| TableOption::new((name, Kind::Integer { min: 0, max: 1 }, whole(0)), Rebuild::No, "Twice.");
    for name in ["NOTED.FILL", "pace"] {
        assert_eq!(db.declare_table_option("other", declared(name)).err().map(|e| e.code), Some(Code::Api), "{name}");
    }
    let clashing = TableOption::new(("NOTED.FILL", Kind::Integer { min: 0, max: 1 }, whole(0)), Rebuild::No, "An engine's.");
    assert_eq!(db.register_engine(Box::new(Clashing(clashing))).err().map(|e| e.code), Some(Code::Api));
}

/// An engine declaring one option, which a plug-in declared.
struct Clashing(TableOption);

impl StorageEngine for Clashing {
    fn name(&self) -> &'static str {
        "clashing"
    }
    fn table_options(&self) -> Vec<TableOption> {
        vec![self.0.clone()]
    }
    fn create_table(&self, _desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(MemoryTable::new()))
    }
}

/// Verifies: [EXTENSION] RULES.md D41, D25: a table whose option no
/// plug-in declares once the database is opened again is opened with it,
/// kept as it was taken when the table was made and listed as NOT DECLARED; it is not set again until
/// declared, and then is taken of its kind; a plug-in declaring it with a
/// kind its value is not of is refused.
#[test]
fn an_option_declared_later_is_kept_as_written() {
    let scratch = Scratch::new("table-options-later");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let mode =
        |words: &'static [&'static str]| TableOption::new(("X.MODE", Kind::Choice(words), Value::Char("SLOW".into())), Rebuild::No, "How.");
    let db = open();
    db.declare_table_option("x", mode(&["SLOW", "FAST"])).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER) WITH (X.MODE = 'fast')").unwrap();
    db.close().unwrap();
    drop(db);
    let db = open();
    assert!(declared(&db, "T").contains(&"rowstore,X.MODE,NULL,FAST,TABLE".to_owned()), "as taken: {:?}", declared(&db, "T"));
    assert_eq!(exec(&db.session("S"), "ALTER TABLE T SET (X.MODE = SLOW)"), Err(Code::SyntaxRule));
    assert_eq!(db.declare_table_option("x", mode(&["SLOW", "STEADY"])).err().map(|e| e.code), Some(Code::Api));
    db.declare_table_option("x", mode(&["SLOW", "FAST"])).unwrap();
    assert!(declared(&db, "T").contains(&"rowstore,X.MODE,x,FAST,TABLE".to_owned()), "{:?}", declared(&db, "T"));
}
