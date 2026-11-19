//! \[EXTENSION\] The kinds of external plug-in of RULES.md D220 to D229,
//! each served by a real process of the example plug-in
//! (`protocol/examples/plugin_example`): rankings, quantizers,
//! collations, system tables, XQuery functions, types, credentials and
//! sinks of events here; storage engines and index methods in
//! `plugin_kinds/engines.rs` and `plugin_kinds/indexes.rs`.

mod engines;
mod indexes;

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{Scratch, exec, failed, rows};
use crate::plugins::fixture::{allow, install, manifest};

/// The objects of the extension KINDS, as its manifest declares them.
pub const KINDS: &str = "\
FUNCTION CRASH() RETURNS INTEGER;
FUNCTION PID() RETURNS BIGINT NOT DETERMINISTIC;
RANKING COUNTED;
QUANTIZER FIRSTS;
COLLATION FOLDED VERSION '1';
SYSTEM TABLE EXAMPLE_FACTS (NAME VARCHAR(40), N INTEGER);
XQUERY FUNCTION '{http://example.com/fn}twice' (INTEGER) RETURNS INTEGER;
TYPE PAIR ORDERED;
STORAGE ENGINE FILES BATCH 100;
INDEX METHOD SORTED SUPPORTS (EQUALITY, RANGE) BATCH 50;
INDEX METHOD NEAR SUPPORTS (NEAREST);
";

/// A scratch directory whose plug-ins hold KINDS at 1.0.0 with `extra`
/// objects, and a database in memory allowed it, KINDS created by U.
pub fn kinds(test: &str, extra: &str) -> (Scratch, Database) {
    let scratch = Scratch::new(test);
    let root = scratch.path("plugins");
    install(&root, ("KINDS", "1.0.0"), &manifest("KINDS", "1.0.0", "", &format!("{KINDS}{extra}")));
    let db = Database::new();
    allow(&db, &root);
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    s.run("CREATE EXTENSION kinds").unwrap();
    s.commit().unwrap();
    (scratch, db)
}

/// The calls made of extension KINDS so far.
pub fn calls(s: &Session) -> u64 {
    rows(s, "SELECT CALLS FROM INFORMATION_SCHEMA.EXTENSIONS WHERE EXTENSION_NAME = 'KINDS'").unwrap()[0].parse().unwrap()
}

/// The SQLSTATE of `sql`'s failure in `s`.
pub fn state(s: &Session, sql: &str) -> String {
    failed(s, sql).sqlstate().as_str().to_owned()
}

/// Verifies: [EXTENSION] RULES.md D220, D221: an extension's ranking is
/// named by FULLTEXT.RANKING, one call a batch of weights; its collation
/// orders and compares a column by sort keys asked a batch at a time, and
/// is refused where a character-wise collation is needed (-103); its
/// quantizer trains, by one call as maintenance finds it due, the
/// codebook a vector index searches by.
#[test]
fn rankings_collations_and_quantizers() {
    let (_scratch, db) = kinds("plugin-kinds-ranking", "");
    let s = db.session("U");
    s.run("CREATE TABLE DOCS (ID INTEGER NOT NULL UNIQUE, BODY VARCHAR(200))").unwrap();
    s.run("INSERT INTO DOCS VALUES (1, 'a dog'), (2, 'a cat'), (3, 'dog eats dog dog'), (4, 'a bird')").unwrap();
    s.commit().unwrap();
    s.run("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext WITH (FULLTEXT.RANKING = COUNTED)").unwrap();
    let top = "SELECT ID FROM DOCS WHERE CONTAINS(BODY, '\"dog\"') = 1 ORDER BY RANK(BODY, '\"dog\"') DESC, ID FETCH FIRST 2 ROWS ONLY";
    assert_eq!(rows(&s, top).unwrap(), ["3", "1"]);
    s.run("CREATE TABLE C (S VARCHAR(20) COLLATE FOLDED)").unwrap();
    s.run("INSERT INTO C VALUES ('b'), ('A'), ('a'), ('B'), ('c')").unwrap();
    let before = calls(&s);
    assert_eq!(rows(&s, "SELECT S FROM C ORDER BY S, S COLLATE UCS_BASIC").unwrap(), ["A", "a", "B", "b", "c"]);
    assert!(calls(&s) - before <= 2, "sort keys asked a batch at a time");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM C WHERE S = 'A'").unwrap(), ["2"]);
    assert_eq!(rows(&s, "SELECT COUNT(DISTINCT S) FROM C").unwrap(), ["3"]);
    assert_eq!(rows(&s, "SELECT S FROM C WHERE S LIKE 'a%'"), Err(Code::SyntaxRule), "LIKE compares characters");
    assert_eq!(rows(&s, "SELECT TRIM(S) FROM C"), Err(Code::SyntaxRule), "so does TRIM");
    s.rollback().unwrap();
    s.run("CREATE TABLE C (S VARCHAR(20) COLLATE FOLDED)").unwrap();
    s.run("CREATE TABLE V (K INTEGER PRIMARY KEY, E VECTOR(4))").unwrap();
    for k in 0..300 {
        s.run(&format!("INSERT INTO V VALUES ({k}, '[{k}, {}, {}, 1]')", k % 7, k % 3)).unwrap();
    }
    let before = calls(&s);
    s.commit().unwrap();
    let options = "DISKANN.METRIC = EUCLIDEAN, DISKANN.QUANTIZER = FIRSTS, DISKANN.PQ_BYTES = 2, DISKANN.TRAIN_AT = 100";
    s.run(&format!("CREATE INDEX V_E ON V (E) USING DISKANN WITH ({options})")).unwrap();
    let approx = "SELECT K FROM V ORDER BY VECTOR_DISTANCE(E, '[120, 1, 0, 1]', EUCLIDEAN) FETCH APPROX FIRST 3 ROWS ONLY";
    let trained = (0..50).any(|_| {
        assert!(rows(&s, approx).unwrap().contains(&"120".to_owned()), "the nearest found, before training and after");
        s.commit().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let now = calls(&s);
        s.commit().unwrap();
        now > before
    });
    assert!(trained, "maintenance asked the quantizer to train");
    let found = rows(&s, approx).unwrap();
    assert!(found.contains(&"120".to_owned()), "searched by its codes, reranked: {found:?}");
}

/// Verifies: [EXTENSION] RULES.md D222: an extension's system table is
/// read as its user sees it, a page of rows a call; its XQuery function
/// is called by its expanded name; both go with DROP EXTENSION.
#[test]
fn system_tables_and_xquery_functions() {
    let (_scratch, db) = kinds("plugin-kinds-system", "");
    let s = db.session("U");
    let before = calls(&s);
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(N) FROM INFORMATION_SCHEMA.EXAMPLE_FACTS WHERE NAME = 'ROW'").unwrap(), ["2500,2500"]);
    assert!((3..=4).contains(&(calls(&s) - before)), "2501 rows in pages of 1024");
    assert_eq!(rows(&s, "SELECT N FROM INFORMATION_SCHEMA.EXAMPLE_FACTS WHERE NAME = 'OWNER'").unwrap(), ["0"]);
    let q = "SELECT XMLCAST(XMLQUERY('declare namespace f = \"http://example.com/fn\"; f:twice(21)' RETURNING SEQUENCE BY REF EMPTY ON EMPTY) AS INTEGER) FROM INFORMATION_SCHEMA.EXAMPLE_FACTS WHERE NAME = 'OWNER'";
    assert_eq!(rows(&s, q).unwrap(), ["42"]);
    s.commit().unwrap();
    s.run("DROP EXTENSION kinds").unwrap();
    assert_eq!(rows(&s, "SELECT * FROM INFORMATION_SCHEMA.EXAMPLE_FACTS"), Err(Code::SyntaxRule));
    let other = "VALUES (XMLCAST(XMLQUERY('declare namespace f = \"http://example.com/fn\"; f:twice(21)' RETURNING SEQUENCE BY REF EMPTY ON EMPTY) AS INTEGER))";
    assert!(rows(&s, other).is_err(), "the function went with it");
}

/// Verifies: [EXTENSION] RULES.md D223, D228: an extension's type is a
/// column's type, made of text by one call and then compared, ordered and
/// indexed by its byte-comparable key with no call; a text it refuses is
/// its SQLSTATE; DROP EXTENSION is refused while a column is of it.
#[test]
fn types_beyond_the_standard() {
    let (_scratch, db) = kinds("plugin-kinds-types", "");
    let s = db.session("U");
    s.run("CREATE TABLE P (K INTEGER, X PAIR)").unwrap();
    s.run("INSERT INTO P VALUES (1, CAST('(2,1)' AS PAIR)), (2, CAST('(1,5)' AS PAIR)), (3, CAST(' (-1, 0) ' AS PAIR))").unwrap();
    let before = calls(&s);
    assert_eq!(rows(&s, "SELECT CAST(X AS VARCHAR(20)) FROM P ORDER BY X").unwrap(), ["(-1,0)", "(1,5)", "(2,1)"]);
    assert_eq!(rows(&s, "SELECT K FROM P WHERE X > CAST('(1,4)' AS PAIR) ORDER BY K").unwrap(), ["1", "2"]);
    assert_eq!(calls(&s) - before, 1, "only the literal's text was asked of the program");
    assert_eq!(state(&s, "VALUES (CAST('(1;2)' AS PAIR))"), "22P02");
    s.commit().unwrap();
    s.run("CREATE INDEX P_X ON P (X)").unwrap();
    assert_eq!(rows(&s, "SELECT K FROM P WHERE X = CAST('(1,5)' AS PAIR)").unwrap(), ["2"]);
    s.commit().unwrap();
    assert_eq!(exec(&s, "DROP EXTENSION kinds CASCADE"), Err(Code::SyntaxRule));
    s.rollback().unwrap();
    s.run("DROP TABLE P CASCADE").unwrap();
    s.run("DROP EXTENSION kinds").unwrap();
}

/// Verifies: [EXTENSION] RULES.md D222: an extension's credentials make
/// each verifier, at most one extension bringing them; its sink is told
/// each event, a batch a call, on a thread of its own.
#[test]
fn credentials_and_diagnostics() {
    let scratch = Scratch::new("plugin-kinds-process");
    let root = scratch.path("plugins");
    let objects = "CREDENTIALS REVERSED;\nDIAGNOSTICS EVENTS BATCH 10;";
    let dir = install(&root, ("KEYS", "1.0.0"), &manifest("KEYS", "1.0.0", "", objects));
    install(&root, ("KEYS2", "1.0.0"), &manifest("KEYS2", "1.0.0", "", "CREDENTIALS REVERSED2;"));
    let db = Database::new();
    allow(&db, &root);
    db.create_user("OWNER", b"").unwrap();
    db.set_owner("OWNER").unwrap();
    let s = db.session("OWNER");
    s.run("CREATE EXTENSION keys").unwrap();
    s.commit().unwrap();
    exec(&s, "CREATE USER ann PASSWORD 'secret'").unwrap();
    assert_eq!(db.verifier("ANN").unwrap().as_deref(), Some(&b"ANN:terces"[..]));
    assert_eq!(exec(&s, "CREATE EXTENSION keys2"), Err(Code::SyntaxRule), "credentials twice");
    s.rollback().unwrap();
    exec(&s, "DROP USER ann").unwrap();
    let log = dir.join("events.log");
    let told = (0..100).find_map(|_| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::read_to_string(&log).ok().filter(|t| t.lines().filter(|l| l.contains("UserChanged")).count() >= 2)
    });
    assert!(told.is_some(), "the sink was told: {:?}", std::fs::read_to_string(&log));
}
