//! The json index, `USING json`, through SQL: lookups of JSON_EXISTS,
//! JSON_VALUE compared equal and JSON_CONTAINS finding the rows a scan
//! finds, of randomized documents; kept as rows change, rolled back,
//! recovered after a crash, of its options, read by a temporary table,
//! and shown by EXPLAIN.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::params::Params;

use crate::common::{Scratch, after_crash, exec, expected, rows};
use turborel::error::Code;

/// How many rows each table holds.
const ROWS: u64 = 1_500;

/// The next of a seeded sequence of pseudo-random numbers.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// A document of a few members, nested objects and arrays, of `state`'s
/// numbers; now and then one that is no JSON text, or null.
fn document(state: &mut u64) -> String {
    let pick = |state: &mut u64, n: u64| next(state) % n;
    match pick(state, 40) {
        0 => return "NULL".to_owned(),
        1 => return "'{ not json'".to_owned(),
        _ => {}
    }
    let kind = ["red", "green", "blue", "red "][usize::try_from(pick(state, 4)).unwrap()];
    let n = pick(state, 20);
    let tags: Vec<String> = (0..pick(state, 4)).map(|_| format!("\"t{}\"", pick(state, 6))).collect();
    let deep = if pick(state, 20) == 0 { format!(r#","a":{{"b":{{"c":{}}}}}"#, pick(state, 5)) } else { String::new() };
    let flag = if pick(state, 2) == 0 { "true" } else { "false" };
    let number = if pick(state, 5) == 0 { format!("{n}.0") } else { n.to_string() };
    format!(
        r#"'{{"kind":"{kind}","n":{number},"flag":{flag},"tags":[{}],"items":[{{"k":"x{}","v":{}}}]{deep}}}'"#,
        tags.join(","),
        pick(state, 4),
        pick(state, 3)
    )
}

/// Schema S's tables INDEXED and PLAIN (ID INTEGER, DOC VARCHAR(400)),
/// each of the same `ROWS` documents of seed `seed`; INDEXED's column DOC
/// with the json index INDEXED_DOC of `with`.
fn documents(db: &Database, seed: u64, with: &str) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE INDEXED (ID INTEGER NOT NULL, DOC VARCHAR(400)) \
         CREATE TABLE PLAIN (ID INTEGER NOT NULL, DOC VARCHAR(400))",
    )
    .unwrap();
    let s = db.session("S");
    let mut state = seed;
    let docs: Vec<String> = (0..ROWS).map(|id| format!("({id}, {})", document(&mut state))).collect();
    for chunk in docs.chunks(100) {
        for table in ["INDEXED", "PLAIN"] {
            exec(&s, &format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, &format!("CREATE INDEX INDEXED_DOC ON INDEXED (DOC) USING json {with}")).unwrap();
    s
}

/// The IDs of `table`'s rows where `condition` holds, in order, or the
/// SQLCODE it raises: of JSON_CONTAINS, of a document no JSON text.
fn found(s: &Session, table: &str, condition: &str) -> Result<Vec<String>, Code> {
    rows(s, &format!("SELECT ID FROM {table} WHERE {condition} ORDER BY ID"))
}

/// Whether the plan of `condition` of INDEXED reads its index.
fn probed(s: &Session, condition: &str) -> bool {
    let plan = s.physical_plan(&format!("SELECT ID FROM INDEXED WHERE {condition}"), &Params::new()).unwrap();
    plan.contains("INDEXED_DOC")
}

/// The conditions each test asks of both tables: member chains, filters
/// of literals and variables, && and ||, JSON_VALUE compared equal, and
/// JSON_CONTAINS. None of a document no JSON text raises, as each catches
/// its errors. Those `SELECTIVE` names hold for few rows, so the planner
/// looks them up; the others may be read by a scan, as costs say.
const LOOKUPS: [&str; 12] = [
    "JSON_EXISTS(DOC, 'lax $.a.b.c')",
    "JSON_EXISTS(DOC, 'lax $.a.b ? (@.c == 3)')",
    "JSON_EXISTS(DOC, 'lax $ ? (@.kind == \"blue\" && @.n == 7)')",
    "JSON_EXISTS(DOC, 'lax $ ? (@.n == 4 || @.n == 13)')",
    "JSON_EXISTS(DOC, 'lax $.items ? (@.k == \"x2\" && @.v == 1)')",
    "JSON_EXISTS(DOC, 'lax $.tags ? (@ == $t)' PASSING 't5' AS \"t\")",
    "JSON_EXISTS(DOC, 'strict $.items[*] ? (@.v == 2)')",
    "JSON_VALUE(DOC, 'lax $.kind') = 'green'",
    "JSON_VALUE(DOC, 'lax $.kind') = 'red '",
    "JSON_VALUE(DOC, 'lax $.flag' RETURNING BOOLEAN) = TRUE",
    "JSON_CONTAINS(DOC, '{\"n\":11,\"tags\":[\"t2\"]}')",
    "JSON_CONTAINS(DOC, '{\"items\":[{\"k\":\"x0\"}],\"flag\":false}')",
];

/// The positions in `LOOKUPS` of the conditions that hold for few rows.
const SELECTIVE: [usize; 4] = [0, 1, 2, 10];

/// Verifies: [EXTENSION] a json index finds, of seeded random documents,
/// exactly the rows a scan of a table without it finds, of member chains,
/// filters of literals and PASSING variables, && and ||, strict paths,
/// JSON_VALUE compared equal, of strings with trailing spaces and of
/// BOOLEAN, and JSON_CONTAINS; each that holds for few rows a lookup of
/// the index.
#[test]
fn lookups_find_what_a_scan_finds() {
    for seed in [7, 1_234_567] {
        let db = Database::new();
        let s = documents(&db, seed, "");
        for at in SELECTIVE {
            assert!(probed(&s, LOOKUPS[at]), "{} reads the index", LOOKUPS[at]);
        }
        for condition in LOOKUPS {
            assert_eq!(found(&s, "INDEXED", condition), found(&s, "PLAIN", condition), "seed {seed}: {condition}");
        }
    }
}

/// Verifies: [EXTENSION] RULES.md D15: a condition that raises of a
/// document no JSON text, ERROR ON ERROR, raises with the index as without
/// it, as a lookup is not made of it; a lookup with it beside another
/// conjunct that raises is not made either; JSON_CONTAINS of a document no
/// JSON text raises, found by the index as a scan finds it.
#[test]
fn errors_are_raised_as_a_scan_raises_them() {
    let db = Database::new();
    let s = documents(&db, 99, "");
    let raising = "JSON_EXISTS(DOC, 'lax $.n' ERROR ON ERROR)";
    assert!(!probed(&s, raising));
    for table in ["INDEXED", "PLAIN"] {
        let sql = format!("SELECT ID FROM {table} WHERE {raising}");
        assert!(s.execute(&sql, &mut Params::new()).is_err(), "{table}");
    }
    let beside = "JSON_EXISTS(DOC, 'lax $.a.b.c') AND 1 / (ID - ID) = 1";
    assert!(!probed(&s, beside), "the other conjunct sees every row");
    let contains = "JSON_CONTAINS(DOC, '{\"n\":11}')";
    assert!(probed(&s, contains));
    for table in ["INDEXED", "PLAIN"] {
        let sql = format!("SELECT ID FROM {table} WHERE {contains}");
        assert!(s.execute(&sql, &mut Params::new()).is_err(), "{table}: the document no JSON text");
    }
}

/// Verifies: [EXTENSION] a json index follows INSERT, UPDATE and DELETE,
/// and a rollback takes its entries back, finding what a scan finds.
#[test]
fn the_index_follows_changes_and_rollbacks() {
    let db = Database::new();
    let s = documents(&db, 3, "");
    let changes = [
        "INSERT INTO {} VALUES (9001, '{\"kind\":\"violet\",\"n\":1}')",
        "UPDATE {} SET DOC = '{\"kind\":\"violet\",\"n\":2}' WHERE ID < 40",
        "DELETE FROM {} WHERE ID BETWEEN 20 AND 30",
    ];
    let violet = "JSON_VALUE(DOC, 'lax $.kind') = 'violet'";
    for change in changes {
        for table in ["INDEXED", "PLAIN"] {
            exec(&s, &change.replace("{}", table)).unwrap();
        }
        assert_eq!(found(&s, "INDEXED", violet), found(&s, "PLAIN", violet), "{change}");
    }
    exec(&s, "ROLLBACK WORK").unwrap();
    assert_eq!(found(&s, "INDEXED", violet), Ok(Vec::new()), "rolled back");
    for change in changes {
        exec(&s, &change.replace("{}", "INDEXED")).unwrap();
    }
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(found(&s, "INDEXED", violet).map(|ids| ids.len()), Ok(30));
}

/// The database in `dir`, recovering after a crash.
fn reopened(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D25: a json index opened again after a
/// crash has its last checkpoint's entries mended by the rows recovery
/// redid: committed changes found, an uncommitted one not.
#[test]
fn the_index_is_recovered_after_a_crash() {
    let scratch = Scratch::new("json-index-recovery");
    let dir = scratch.path("db");
    let db = reopened(&dir);
    let s = documents(&db, 11, "");
    db.checkpoint().unwrap();
    exec(&s, "INSERT INTO INDEXED VALUES (9001, '{\"zebra\":1}')").unwrap();
    exec(&s, "UPDATE INDEXED SET DOC = '{\"zebra\":2}' WHERE ID = 5").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "INSERT INTO INDEXED VALUES (9002, '{\"zebra\":3}')").unwrap();
    drop(db);
    let zebra = "JSON_EXISTS(DOC, 'lax $.zebra')";
    for _ in 0..2 {
        let db = reopened(&dir);
        let s = db.session("S");
        assert!(probed(&s, zebra));
        assert_eq!(found(&s, "INDEXED", zebra), expected(&["5", "9001"]));
        exec(&s, "COMMIT WORK").unwrap();
        db.checkpoint().unwrap();
    }
    drop(s);
}

/// Verifies: [EXTENSION] RULES.md D41: JSON.PATHS_ONLY and JSON.MAX_DEPTH
/// make an index posting less, whose lookups still find what a scan
/// finds; ALTER INDEX of either makes it anew; a value out of range is
/// refused.
#[test]
fn options_post_less_and_find_alike() {
    let db = Database::new();
    let s = documents(&db, 5, "WITH (JSON.PATHS_ONLY = 'ON', JSON.MAX_DEPTH = 1)");
    for condition in LOOKUPS {
        assert_eq!(found(&s, "INDEXED", condition), found(&s, "PLAIN", condition), "{condition}");
    }
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "ALTER INDEX INDEXED_DOC SET (JSON.PATHS_ONLY = 'OFF')").unwrap();
    for condition in LOOKUPS {
        assert_eq!(found(&s, "INDEXED", condition), found(&s, "PLAIN", condition), "{condition}");
    }
    exec(&s, "COMMIT WORK").unwrap();
    assert!(exec(&s, "ALTER INDEX INDEXED_DOC SET (JSON.MAX_DEPTH = 1000)").is_err());
}

/// Verifies: [EXTENSION] EXPLAIN shows a json lookup as the call it
/// answers, of its PASSING values; a lookup of a parameter's value is made.
#[test]
fn explain_shows_the_lookup() {
    let db = Database::new();
    let s = documents(&db, 13, "");
    let plan = s
        .physical_plan("SELECT ID FROM INDEXED WHERE JSON_EXISTS(DOC, 'lax $.a.b ? (@.c == $c)' PASSING 3 AS \"c\")", &Params::new())
        .unwrap();
    assert!(plan.contains("INDEXED_DOC") && plan.contains("JSON_EXISTS"), "{plan}");
    let params = Params::new().declare("K", turborel::value::DataType::varying(10), turborel::value::Value::Char("violet".into()));
    let plan = s.physical_plan("SELECT ID FROM INDEXED WHERE JSON_VALUE(DOC, 'lax $.kind') = K", &params).unwrap();
    assert!(plan.contains("INDEXED_DOC"), "a host parameter is probed: {plan}");
}

/// Verifies: [EXTENSION] a json index of a column of a type no JSON
/// document is, or of two columns, is refused.
#[test]
fn only_a_column_of_documents_is_indexed() {
    let db = Database::new();
    let s = documents(&db, 17, "");
    assert!(exec(&s, "CREATE INDEX BAD ON PLAIN (ID) USING json").is_err());
    assert!(exec(&s, "CREATE INDEX BAD ON PLAIN (ID, DOC) USING json").is_err());
}

/// Verifies: [EXTENSION] RULES.md D49: a temporary table's json index
/// holds each session's own rows, and its lookups find them alone.
#[test]
fn a_temporary_table_has_a_json_index_of_its_own_rows() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE GLOBAL TEMPORARY TABLE NOTES (ID INTEGER, DOC VARCHAR(100)) ON COMMIT PRESERVE ROWS")
        .unwrap();
    let (one, two) = (db.session("S"), db.session("S"));
    exec(&one, "CREATE INDEX NOTES_DOC ON NOTES (DOC) USING json").unwrap();
    exec(&one, "INSERT INTO NOTES VALUES (1, '{\"a\":1}'), (2, '{\"b\":1}')").unwrap();
    exec(&two, "INSERT INTO NOTES VALUES (3, '{\"a\":2}')").unwrap();
    assert_eq!(found(&one, "NOTES", "JSON_EXISTS(DOC, 'lax $.a')"), expected(&["1"]));
    assert_eq!(found(&two, "NOTES", "JSON_EXISTS(DOC, 'lax $.a')"), expected(&["3"]));
}

/// Verifies: [EXTENSION] RULES.md D34: a lookup of a filter of `&&` over
/// `||`, and of `||` over `&&`, finds by its rarest conjunct the rows a
/// scan finds, each other part checked of the documents found.
#[test]
fn nested_and_or_filters_find_what_a_scan_finds() {
    let db = Database::new();
    let s = documents(&db, 11, "");
    for condition in [
        "JSON_EXISTS(DOC, 'lax $ ? (@.a.b.c == 1 && (@.n == 4 || @.kind == \"red\"))')",
        "JSON_EXISTS(DOC, 'lax $ ? (@.a.b.c == 2 && ((@.n == 4 && @.flag == true) || @.kind == \"blue\"))')",
    ] {
        assert!(probed(&s, condition), "{condition} reads the index");
        assert_eq!(found(&s, "INDEXED", condition), found(&s, "PLAIN", condition), "{condition}");
    }
}
