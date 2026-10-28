//! Index settings (RULES.md D34, D41): CREATE, DROP and ALTER INDEX, with
//! the options an index method declares for its indexes, and those other
//! plug-ins declare for every index, listed in INFORMATION_SCHEMA.INDEXES
//! and INDEX_OPTIONS.

use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, Result};
use turborel::index::{BTreeMethod, Index, IndexMethod};
use turborel::settings::Kind;
use turborel::storage::{Rebuild, TableOption};
use turborel::value::{CompOp, DataType, Value};

use crate::common::{Scratch, exec, os, rows};
use crate::table_settings::{Calls, Noted, noted, whole};

/// The indexes of table T of schema S, as INDEXES lists them, and their
/// options, as INDEX_OPTIONS does: index, method, option, value, source.
fn indexes(db: &Database) -> (Vec<String>, Vec<String>) {
    let s = db.session("S");
    let trim = |rows: Vec<String>| rows.into_iter().map(|r| r.replace(' ', "")).collect::<Vec<_>>();
    let listed = "SELECT INDEX_NAME, METHOD_NAME, IS_UNIQUE, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES \
                  WHERE TABLE_SCHEMA = 'S' AND TABLE_NAME = 'T' AND IS_UNIQUE = 'NO' ORDER BY INDEX_NAME";
    let options = "SELECT INDEX_NAME, METHOD_NAME, OPTION_NAME, OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.INDEX_OPTIONS \
                   WHERE TABLE_SCHEMA = 'S' AND TABLE_NAME = 'T' ORDER BY INDEX_NAME, OPTION_NAME";
    (trim(rows(&s, listed).unwrap()), trim(rows(&s, options).unwrap()))
}

/// `noted`, with schema S's table T (K INTEGER NOT NULL UNIQUE, V CHAR(4))
/// of 20 rows.
fn indexed() -> (Database, Calls) {
    let (db, calls) = noted();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(4))").unwrap();
    let s = db.session("S");
    for k in 0..20 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{}')", k % 5)).unwrap();
    }
    s.commit().unwrap();
    calls.taken();
    (db, calls)
}

/// Verifies: [EXTENSION] RULES.md D34, D41: CREATE INDEX makes an index on
/// columns of a table, of the B-link tree's method unless USING names
/// another, in any case, WITH options of the method's, which the method's
/// index is made with (`IndexMethod::create_on`); the planner probes it,
/// INDEXES and INDEX_OPTIONS list it, and DROP INDEX takes it away. They
/// run outside a transaction (SQLCODE -917), by the owner of the table's
/// schema (-104); a column, method, index or option not there, a name
/// taken, an option twice, a value not of its kind, or an index of a
/// UNIQUE constraint dropped is a Syntax Rule broken (-103).
#[test]
fn indexes_are_created_and_dropped_in_sql() {
    let (db, calls) = indexed();
    let s = db.session("S");
    exec(&s, "CREATE INDEX T_V ON T (V)").unwrap();
    exec(&s, "CREATE INDEX T_KV ON T (K, V) USING Noted WITH (FILL = 70, SHAPE = TALL)").unwrap();
    assert_eq!(calls.taken(), ["index T  / FILL=70,SHAPE=TALL"]);
    let (listed, options) = indexes(&db);
    assert_eq!(listed, ["T_KV,noted,NO,K,V", "T_V,btree,NO,V"]);
    let constraint = ["S.T.UNIQUE1,noted,FILL,90,DEFAULT", "S.T.UNIQUE1,noted,SHAPE,WIDE,DEFAULT"];
    assert_eq!(
        options,
        [constraint.as_slice(), &["T_KV,noted,FILL,70,INDEX", "T_KV,noted,SHAPE,TALL,INDEX"]].concat(),
        "the UNIQUE constraint's too"
    );
    let plan = s.explain("SELECT K FROM T WHERE V = 'v1'", &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("T_V"), "{plan}");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE V = 'v1' ORDER BY K"), Ok(["1", "6", "11", "16"].map(String::from).to_vec()));
    s.commit().unwrap();
    let refused = [
        ("CREATE INDEX X ON T (NOPE)", Code::SyntaxRule),
        ("CREATE INDEX X ON T (V) USING NOPE", Code::SyntaxRule),
        ("CREATE INDEX T_V ON T (K)", Code::SyntaxRule),
        ("CREATE INDEX X ON T (V) USING NOTED WITH (SPEED = 1)", Code::SyntaxRule),
        ("CREATE INDEX X ON T (V) USING NOTED WITH (FILL = 1, FILL = 2)", Code::SyntaxRule),
        ("CREATE INDEX X ON T (V) USING NOTED WITH (FILL = 0)", Code::SyntaxRule),
        ("CREATE INDEX X ON NOPE (V)", Code::SyntaxRule),
        ("DROP INDEX NOPE", Code::SyntaxRule),
    ];
    for (sql, expected) in refused {
        assert_eq!(exec(&s, sql), Err(expected), "{sql}");
    }
    let unique = rows(&s, "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE TABLE_NAME = 'T' AND IS_UNIQUE = 'YES'").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, &format!("DROP INDEX \"{}\"", unique[0].trim())), Err(Code::SyntaxRule), "a constraint's");
    assert_eq!(exec(&db.session("R"), "CREATE INDEX X ON S.T (V)"), Err(Code::Privilege));
    assert_eq!(exec(&db.session("R"), "DROP INDEX T_V"), Err(Code::Privilege));
    s.run("INSERT INTO T VALUES (100, 'x')").unwrap();
    assert_eq!(exec(&s, "CREATE INDEX X ON T (V)"), Err(Code::TransactionState), "in a transaction");
    s.rollback().unwrap();
    exec(&s, "DROP INDEX T_V").unwrap();
    assert_eq!(indexes(&db).0, ["T_KV,noted,NO,K,V"]);
}

/// Verifies: [EXTENSION] RULES.md D41: ALTER INDEX ... SET changes an
/// index option made to change in place, the index told; one fixed when
/// the index is made, by building the index again with it; DEFAULT makes an option its default again; only the owner of the
/// table's schema alters it (-104). A database kept in a directory has the
/// options once opened again.
#[test]
fn index_options_change_in_place_and_survive_the_database_closing() {
    let scratch = Scratch::new("index-options");
    let dir = scratch.path("db");
    let calls = Calls::default();
    let open = || {
        let db = Database::new();
        db.register_index_method(Box::new(Noted(BTreeMethod::in_memory(), calls.clone()))).unwrap();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("S");
    exec(&s, "CREATE INDEX T_K ON T (K) USING NOTED WITH (FILL = 60)").unwrap();
    exec(&s, "ALTER INDEX T_K SET (FILL = 40, SHAPE = DEFAULT)").unwrap();
    assert_eq!(calls.taken(), ["index T  / FILL=60", "index told T  / FILL=40"]);
    exec(&s, "ALTER INDEX T_K SET (SHAPE = TALL)").unwrap();
    assert_eq!(calls.taken(), ["index T  / FILL=40,SHAPE=TALL"], "built again");
    assert_eq!(exec(&s, "ALTER INDEX T_K SET (SPEED = 1)"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "ALTER INDEX NOPE SET (FILL = 1)"), Err(Code::SyntaxRule));
    assert_eq!(exec(&db.session("R"), "ALTER INDEX T_K SET (FILL = 1)"), Err(Code::Privilege));
    assert_eq!(indexes(&db).1, ["T_K,noted,FILL,40,INDEX", "T_K,noted,SHAPE,TALL,INDEX"]);
    drop(s);
    db.close().unwrap();
    drop(db);
    let db = open();
    assert_eq!(indexes(&db).1, ["T_K,noted,FILL,40,INDEX", "T_K,noted,SHAPE,TALL,INDEX"]);
    exec(&db.session("S"), "ALTER INDEX T_K SET (FILL = DEFAULT)").unwrap();
    assert_eq!(indexes(&db).1, ["T_K,noted,FILL,90,DEFAULT", "T_K,noted,SHAPE,TALL,INDEX"]);
}

/// Verifies: [EXTENSION] RULES.md D41, D27: an index built again for an
/// option fixed when it is made is built while transactions write its
/// table, as CREATE INDEX builds one: a transaction that wrote the table
/// before is refused at its commit (SQLCODE -911), to run again; the index
/// keeps its name, and the planner probes it, for the rows it had and those
/// written since; the index of a UNIQUE constraint built again enforces it
/// still. Only the index is listed, never the one being built.
#[test]
fn an_index_is_built_again_while_its_table_is_written() {
    let (db, calls) = indexed();
    let s = db.session("S");
    exec(&s, "CREATE INDEX T_V ON T (V) USING NOTED").unwrap();
    let early = db.session("S");
    early.run("INSERT INTO T VALUES (100, 'v1')").unwrap();
    exec(&s, "ALTER INDEX T_V SET (SHAPE = TALL)").unwrap();
    assert_eq!(early.commit().map_err(|e| e.code), Err(Code::Conflict), "it wrote T before");
    s.run("INSERT INTO T VALUES (101, 'v1')").unwrap();
    s.commit().unwrap();
    let plan = s.explain("SELECT K FROM T WHERE V = 'v1'", &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("T_V"), "{plan}");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE V = 'v1' ORDER BY K"), Ok(["1", "6", "11", "16", "101"].map(String::from).to_vec()));
    s.commit().unwrap();
    exec(&s, "ALTER INDEX \"S.T.UNIQUE1\" SET (SHAPE = TALL)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (101, 'x')"), Err(Code::Constraint), "its UNIQUE constraint still");
    s.rollback().unwrap();
    assert_eq!(indexes(&db).0, ["T_V,noted,NO,V"]);
    let unique = rows(&s, "SELECT INDEX_NAME, IS_UNIQUE FROM INFORMATION_SCHEMA.INDEXES WHERE IS_UNIQUE = 'YES'").unwrap();
    assert_eq!(unique.len(), 1, "{unique:?}");
    let shown: Vec<String> = calls.taken().into_iter().filter(|c| c.starts_with("index T")).collect();
    assert_eq!(shown, ["index T ", "index T  / SHAPE=TALL", "index T  / SHAPE=TALL"], "each built again with its options");
}

/// Verifies: [EXTENSION] RULES.md D41, D27: an index of a table whose
/// engine leaves concurrency to the coordinator, so keeps no versions, is
/// built again under its table's exclusive lock, as CREATE INDEX builds
/// one: with the option given, in its place and name, holding every row.
#[test]
fn an_index_of_a_locked_table_is_built_again_under_its_lock() {
    let (db, calls) = noted();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, V CHAR(4)) USING TUNED").unwrap();
    let s = db.session("S");
    for k in 0..40 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{}')", k % 10)).unwrap();
    }
    s.commit().unwrap();
    exec(&s, "CREATE INDEX T_V ON T (V) USING NOTED").unwrap();
    calls.taken();
    exec(&s, "ALTER INDEX T_V SET (SHAPE = TALL)").unwrap();
    assert_eq!(calls.taken(), ["index T  / SHAPE=TALL"]);
    assert_eq!(
        indexes(&db),
        (vec!["T_V,noted,NO,V".to_owned()], vec!["T_V,noted,FILL,90,DEFAULT".to_owned(), "T_V,noted,SHAPE,TALL,INDEX".to_owned()])
    );
    let plan = s.explain("SELECT K FROM T WHERE V = 'v1'", &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("T_V"), "{plan}");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE V = 'v1' ORDER BY K"), Ok(["1", "11", "21", "31"].map(String::from).to_vec()));
}

/// Verifies: [EXTENSION] RULES.md D41: an index is built again under a
/// working name, "<name> (rebuilt)"; if another index has that name, it is
/// not built again (SQLCODE -910), and stays as it was.
#[test]
fn an_index_is_not_built_again_over_another() {
    let (db, _) = indexed();
    let s = db.session("S");
    exec(&s, "CREATE INDEX T_V ON T (V) USING NOTED").unwrap();
    exec(&s, "CREATE INDEX \"T_V (rebuilt)\" ON T (K)").unwrap();
    assert_eq!(exec(&s, "ALTER INDEX T_V SET (SHAPE = TALL)"), Err(Code::Api));
    let (listed, options) = indexes(&db);
    assert_eq!(listed, ["T_V,noted,NO,V", "T_V(rebuilt),btree,NO,K"]);
    assert!(options.contains(&"T_V,noted,SHAPE,WIDE,DEFAULT".to_owned()), "{options:?}");
    exec(&s, "DROP INDEX \"T_V (rebuilt)\"").unwrap();
    exec(&s, "ALTER INDEX T_V SET (SHAPE = TALL)").unwrap();
}

/// The options of index `index` of table S.T, with who declared each:
/// option, declarer, value and source.
fn declared(db: &Database, index: &str) -> Vec<String> {
    let sql = format!(
        "SELECT OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.INDEX_OPTIONS \
         WHERE TABLE_SCHEMA = 'S' AND INDEX_NAME = '{index}' ORDER BY OPTION_NAME"
    );
    rows(&db.session("S"), &sql).unwrap().into_iter().map(|r| r.replace(' ', "")).collect()
}

/// The options "audit" declares for every index: AUDIT.LEVEL, changed in
/// place, and AUDIT.TRAIL, fixed when the index is made.
fn audit_options() -> [TableOption; 2] {
    [
        TableOption::new(("AUDIT.LEVEL", Kind::Integer { min: 0, max: 3 }, whole(1)), Rebuild::No, "How closely."),
        TableOption::new(("AUDIT.TRAIL", Kind::Choice(&["ON", "OFF"]), Value::Char("OFF".into())), Rebuild::Yes, "Whether kept."),
    ]
}

/// Verifies: [EXTENSION] RULES.md D41, `Database::declare_index_option`: a
/// plug-in declares options of every index, of any method, set and changed
/// as a method's own are: the method's index is made with them
/// (`IndexMethod::create_on`) and told of a change made in place, and
/// built again for one fixed when it is made; INDEX_OPTIONS lists who declared
/// each. An option is declared once, by a method or by a plug-in.
#[test]
fn plug_ins_declare_options_of_every_index() {
    let (db, calls) = indexed();
    for option in audit_options() {
        db.declare_index_option("audit", option).unwrap();
    }
    let s = db.session("S");
    exec(&s, "CREATE INDEX T_V ON T (V) WITH (AUDIT.LEVEL = 2)").unwrap();
    exec(&s, "CREATE INDEX T_KV ON T (K, V) USING NOTED WITH (FILL = 50, AUDIT.TRAIL = ON)").unwrap();
    assert_eq!(calls.taken(), ["index T  / AUDIT.TRAIL=ON,FILL=50"], "the method reads them");
    assert_eq!(declared(&db, "T_V"), ["AUDIT.LEVEL,audit,2,INDEX", "AUDIT.TRAIL,audit,OFF,DEFAULT"]);
    exec(&s, "ALTER INDEX T_KV SET (AUDIT.LEVEL = 3)").unwrap();
    assert_eq!(calls.taken(), ["index told T  / AUDIT.LEVEL=3,AUDIT.TRAIL=ON,FILL=50"]);
    exec(&s, "ALTER INDEX T_KV SET (AUDIT.TRAIL = OFF)").unwrap();
    assert_eq!(calls.taken(), ["index T  / AUDIT.LEVEL=3,AUDIT.TRAIL=OFF,FILL=50"], "fixed when it is made: built again");
    let listed = declared(&db, "T_KV");
    assert!(listed.contains(&"FILL,noted,50,INDEX".to_owned()) && listed.contains(&"AUDIT.LEVEL,audit,3,INDEX".to_owned()), "{listed:?}");
    let again = |name: &str| TableOption::new((name, Kind::Integer { min: 0, max: 1 }, whole(0)), Rebuild::No, "Twice.");
    for name in ["AUDIT.LEVEL", "fill"] {
        assert_eq!(db.declare_index_option("other", again(name)).err().map(|e| e.code), Some(Code::Api), "{name}");
    }
    let clashing = Auditing(again("AUDIT.LEVEL"));
    assert_eq!(db.register_index_method(Box::new(clashing)).err().map(|e| e.code), Some(Code::Api), "a plug-in's name");
    db.declare_table_option("other", again("FILL")).unwrap();
}

/// An index method declaring one option, which a plug-in declared.
struct Auditing(TableOption);

impl IndexMethod for Auditing {
    fn name(&self) -> &'static str {
        "auditing"
    }
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn index_options(&self) -> Vec<TableOption> {
        vec![self.0.clone()]
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        BTreeMethod::in_memory().create(columns)
    }
}

/// Verifies: [EXTENSION] RULES.md D41, D25: an index whose option no
/// plug-in declares once the database is opened again is made with it,
/// kept as it was recorded and listed as NOT DECLARED; it is not set again until
/// declared, and then is taken of its kind; a declaration its value is not
/// of is refused.
#[test]
fn an_index_option_declared_later_is_kept_as_written() {
    let scratch = Scratch::new("index-options-later");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let trail = |words: &'static [&'static str]| {
        TableOption::new(("AUDIT.TRAIL", Kind::Choice(words), Value::Char("OFF".into())), Rebuild::No, "Whether kept.")
    };
    let db = open();
    db.declare_index_option("audit", trail(&["ON", "OFF"])).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER)").unwrap();
    exec(&db.session("S"), "CREATE INDEX T_K ON T (K) WITH (AUDIT.TRAIL = 'on')").unwrap();
    db.close().unwrap();
    drop(db);
    let db = open();
    assert_eq!(declared(&db, "T_K"), ["AUDIT.TRAIL,NULL,ON,INDEX"], "as it was recorded");
    assert_eq!(exec(&db.session("S"), "ALTER INDEX T_K SET (AUDIT.TRAIL = OFF)"), Err(Code::SyntaxRule));
    assert_eq!(db.declare_index_option("audit", trail(&["YES", "NO"])).err().map(|e| e.code), Some(Code::Api));
    db.declare_index_option("audit", trail(&["ON", "OFF"])).unwrap();
    assert_eq!(declared(&db, "T_K"), ["AUDIT.TRAIL,audit,ON,INDEX"]);
}
