//! The vector index methods' files: recovery after a crash, reopening an
//! index by its locator, a locator of an unknown version, an encrypted
//! table's index (RULES.md D48) and a temporary table's.

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::Code;
use turborel::index::{DiskAnnMethod, HnswMethod, IndexMethod, IvfMethod};
use turborel::vector::{Element, Layout, Shape, typed};

use super::fixture::{DIMS, METHODS, database, insert, literal, nearest_sql, on_files, populate, query_of, recall, register};
use crate::common::{RunLength, Scramble, Scratch, after_crash, rows};

/// T's rows nearest query 0, as FETCH APPROX answers.
fn first(s: &turborel::db::Session) -> Vec<String> {
    rows(s, &nearest_sql("L2_DISTANCE", 0, ("APPROX", ""))).unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, D95, D96, D97, D98: a database
/// of files recovers each method's index: committed changes after its
/// checkpoint are kept and an open one's undone, mended by the log; then
/// reopened again by its locator, it answers as before, its recall kept.
#[test]
fn each_index_recovers_and_reopens_d25() {
    for method in METHODS {
        let scratch = Scratch::new(&format!("vector-index-{method}"));
        let db = on_files(&scratch);
        populate(&db, 1500);
        let s = db.session("S");
        s.run(&format!("CREATE INDEX T_V ON T (V) USING {method} WITH ({}.METRIC = EUCLIDEAN)", method.to_ascii_uppercase())).unwrap();
        s.commit().unwrap();
        db.checkpoint().unwrap();
        let nearest = first(&s).first().cloned().unwrap();
        s.run(&format!("DELETE FROM T WHERE ID = {nearest}")).unwrap();
        s.run(&format!("INSERT INTO T VALUES (7777, {}, 0)", literal(&query_of(0)))).unwrap();
        s.commit().unwrap();
        let open = db.session("S");
        open.run(&format!("INSERT INTO T VALUES (8888, {}, 0)", literal(&query_of(0)))).unwrap();
        db.checkpoint().unwrap();
        drop((s, db));
        let recovered = Database::new();
        register(&recovered);
        let opened = recovered.open_files(&after_crash(), &scratch.path("db"), &OpenOptions::default()).unwrap();
        assert!(matches!(opened, Opened::Recovered(_)), "{method}");
        let s = recovered.session("S");
        let found = first(&s);
        assert_eq!(found.first().map(String::as_str), Some("7777"), "{method}: {found:?}");
        assert!(!found.contains(&nearest) && !found.contains(&"8888".to_owned()), "{method}: {found:?}");
        assert!(recall(&s, "L2_DISTANCE", "") >= 0.9, "{method}");
        recovered.checkpoint().unwrap();
        drop((s, recovered, open));
        let reopened = on_files(&scratch);
        let s = reopened.session("S");
        assert_eq!(first(&s).first().map(String::as_str), Some("7777"), "{method}");
        insert(&s, (2000, 2100));
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), ["1601"], "{method}");
    }
}

/// Verifies: [DESIGN] RULES.md D95: a locator of
/// another version, or of a column of other dimensions, is refused as
/// corrupt (SQLCODE -916), never read as this version's.
#[test]
fn a_locator_of_an_unknown_version_is_corrupt() {
    let column =
        typed(Shape { dims: Some(u32::try_from(DIMS).unwrap()), element: Some(Element::Float32), layout: Some(Layout::Dense) }).unwrap();
    let methods: [Box<dyn IndexMethod>; 3] =
        [Box::new(DiskAnnMethod::in_memory()), Box::new(HnswMethod::in_memory()), Box::new(IvfMethod::in_memory())];
    for method in methods {
        let made = method.create(std::slice::from_ref(&column)).unwrap();
        let locator = made.locator();
        assert!(method.open(std::slice::from_ref(&column), &locator).is_ok(), "{}", method.name());
        let mut later = locator.clone();
        later[0] = 2;
        let opened = method.open(std::slice::from_ref(&column), &later);
        assert_eq!(opened.err().map(|e| e.code), Some(Code::Corrupt), "{}", method.name());
        let mut other = locator.clone();
        other[2] ^= 1;
        let opened = method.open(std::slice::from_ref(&column), &other);
        assert_eq!(opened.err().map(|e| e.code), Some(Code::Corrupt), "{}", method.name());
    }
}

/// Verifies: [EXTENSION] RULES.md D48: a vector index keeps its table's
/// vectors in the clear, so of an encrypted table it is refused (SQLCODE
/// -103) where the database's files are not encrypted.
#[test]
fn an_encrypted_tables_vectors_are_not_indexed_in_the_clear_d48() {
    let scratch = Scratch::new("vector-index-sealed");
    let db = Database::new();
    register(&db);
    db.register_encoding(Box::new(RunLength)).unwrap();
    db.register_encoding(Box::new(Scramble)).unwrap();
    db.open_files(&after_crash(), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (ID INTEGER, V VECTOR({DIMS})) WITH (ENCODINGS = 'test scramble')"))
        .unwrap();
    for method in METHODS {
        assert_eq!(db.create_index("T_V", "S.T", &["V"], method).err().map(|e| e.code), Some(Code::SyntaxRule), "{method}");
    }
}

/// Verifies: [EXTENSION] RULES.md D94: a temporary table's vector
/// index holds each session's own rows, and answers each alone.
#[test]
fn a_temporary_tables_index_is_each_sessions_own() {
    for method in METHODS {
        let db = database(0);
        db.schema(&format!(
            "CREATE SCHEMA AUTHORIZATION P CREATE GLOBAL TEMPORARY TABLE T (ID INTEGER, V VECTOR({DIMS}), C INTEGER) ON COMMIT PRESERVE ROWS"
        ))
        .unwrap();
        db.create_index("T_V", "P.T", &["V"], method).unwrap();
        let (a, b) = (db.session("P"), db.session("P"));
        insert(&a, (0, 300));
        insert(&b, (1000, 1300));
        let ids = |s: &turborel::db::Session| -> Vec<i64> {
            rows(s, &nearest_sql("COSINE_DISTANCE", 0, ("APPROX", ""))).unwrap().iter().map(|r| r.parse().unwrap()).collect()
        };
        assert!(ids(&a).iter().all(|id| (0..300).contains(id)), "{method}");
        assert!(ids(&b).iter().all(|id| (1000..1300).contains(id)), "{method}");
        assert_eq!(ids(&a).len(), 10, "{method}");
    }
}
