//! Label and reachability indexes of a graph's element tables (RULES.md
//! D115, D116): CREATE LABEL INDEX and CREATE REACHABILITY INDEX ON
//! GRAPH, the CREATE INDEX statements they are, what EXPLAIN names of
//! them, and a GRAPH_TABLE read through them giving the rows it gives
//! without them, after changes, maintenance, reopened and after a crash.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::params::Params;

use super::{BANK, FIXTURE, rows};
use crate::common::{Scratch, after_crash, os};

/// Searches a reachability index prunes, ends kept by a WHERE of their
/// own, followed forward and backward, of each selector and restrictor
/// kind, and an end of no WHERE and a lateral one, which it does not.
const SEARCHES: &[&str] = &[
    "SELECT A, B, N FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a:Account)-[t:Transfer]->+(b:Account WHERE b.owner = 'Charles') \
     COLUMNS (a.id AS A, b.id AS B, COUNT(t.*) AS N))",
    "SELECT A, B, N FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a)<-[t:Transfer]-{1,4}(b WHERE b.is_blocked = 'yes') \
     COLUMNS (a.id AS A, b.id AS B, COUNT(t.*) AS N))",
    "SELECT A, B FROM GRAPH_TABLE (BANK MATCH ANY (a)-[t:Transfer]->*(b WHERE b.id IN ('a1', 'a7')) COLUMNS (a.id AS A, b.id AS B))",
    "SELECT A, N FROM GRAPH_TABLE (BANK MATCH TRAIL (a)-[t:Transfer]->+(b WHERE b.id = 'a5') COLUMNS (a.id AS A, COUNT(t.*) AS N))",
    "SELECT A, B, N FROM GRAPH_TABLE (BANK MATCH ALL SHORTEST (a)-[:Transfer]->(m)-[t:Transfer]->{0,3}(b WHERE b.owner < 'D') \
     COLUMNS (a.id AS A, b.id AS B, COUNT(t.*) AS N))",
    "SELECT A, B FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a)-[t:Transfer]->+(b) COLUMNS (a.id AS A, b.id AS B))",
    "SELECT X.ID, G.B FROM ACCOUNT AS X, GRAPH_TABLE (BANK MATCH ANY SHORTEST (a)-[t:Transfer]->+(b WHERE b.id = X.ID) \
     COLUMNS (a.id AS B)) AS G",
];

/// Label tests of every operator, of labels one table has and labels
/// tables share, on vertices and edges.
const LABEL_TESTS: &[&str] = &[
    "SELECT X FROM GRAPH_TABLE (BANK MATCH (x:Country) COLUMNS (x.id AS X))",
    "SELECT X FROM GRAPH_TABLE (BANK MATCH (x:!Account) COLUMNS (x.id AS X))",
    "SELECT X FROM GRAPH_TABLE (BANK MATCH (x:Country&City) COLUMNS (x.id AS X))",
    "SELECT X FROM GRAPH_TABLE (BANK MATCH (x:Phone|City) COLUMNS (x.id AS X))",
    "SELECT X, E FROM GRAPH_TABLE (BANK MATCH (x:%)-[e:IsLocatedIn|!Transfer]->(y:Country) COLUMNS (x.id AS X, e.id AS E))",
    "SELECT X, Y FROM GRAPH_TABLE (BANK MATCH (x)-[e:%]-{2}(y:Account&!Phone) COLUMNS (x.id AS X, y.id AS Y))",
];

/// The rows, or the SQLCODE, of each of `queries` in `s`.
fn answers(s: &Session, queries: &[&str]) -> Vec<Result<Vec<String>, i32>> {
    queries
        .iter()
        .map(|q| {
            let mut c = s.open_cursor(q).map_err(|e| e.sqlcode())?;
            let mut out = Vec::new();
            while let Some(row) = c.fetch().map_err(|e| e.sqlcode())? {
                out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
            }
            out.sort();
            Ok(out)
        })
        .collect()
}

/// The fixture in `db`, an account of no transfers added, its graph
/// BANK committed; a session of A.
fn fixture(db: &Database) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    for sql in FIXTURE {
        s.run(sql).unwrap();
    }
    s.run("INSERT INTO ACCOUNT VALUES ('a7', 'Lone', 'no')").unwrap();
    s.run(BANK).unwrap();
    s.run("COMMIT WORK").unwrap();
    s
}

/// The label and reachability indexes of BANK made in `s`.
fn indexed(s: &Session) {
    s.run("CREATE LABEL INDEX ON GRAPH BANK").unwrap();
    s.run("CREATE REACHABILITY INDEX ON GRAPH BANK EDGE TABLE TRANSFER").unwrap();
}

/// The names of the indexes of `method`, read in a transaction of its own.
fn indexes_of(s: &Session, method: &str) -> Vec<String> {
    let names = rows(s, &format!("SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE METHOD_NAME = '{method}'"));
    s.run("COMMIT WORK").unwrap();
    names
}

/// Runs `sql` in both sessions, committed.
fn both(plain: &Session, kept: &Session, sql: &str) {
    for s in [plain, kept] {
        s.run(sql).unwrap();
        s.run("COMMIT WORK").unwrap();
    }
}

/// Checks the sessions give the same answers, which are `want` if given.
fn same(plain: &Session, kept: &Session, when: &str) {
    for queries in [SEARCHES, LABEL_TESTS] {
        assert_eq!(answers(kept, queries), answers(plain, queries), "{when}");
    }
}

/// Verifies: [EXTENSION] RULES.md D115, D116: CREATE LABEL INDEX makes an
/// index of the labels method of each base element table's key, named of
/// the graph or of the name written, or of the one table it names;
/// CREATE REACHABILITY INDEX one of the reachability method of an edge
/// table's ends; plain CREATE INDEX of either method too. A graph or an
/// element table the graph has not is refused, GRF-0001 and GRF-0012,
/// and an edge table named as a vertex table, a syntax error -102.
#[test]
fn graph_index_statements_make_indexes() {
    let db = Database::new();
    let s = fixture(&db);
    s.run("CREATE LABEL INDEX ON GRAPH BANK").unwrap();
    assert_eq!(indexes_of(&s, "labels").len(), 8);
    assert!(indexes_of(&s, "labels").contains(&"BANK_TRANSFER_LABELS".to_owned()));
    s.run("CREATE LABEL INDEX L ON GRAPH BANK").unwrap();
    assert!(indexes_of(&s, "labels").contains(&"L_HAS_PHONE".to_owned()));
    s.run("CREATE LABEL INDEX ONE ON GRAPH BANK VERTEX TABLE ACCOUNT").unwrap();
    let e = s.run("CREATE LABEL INDEX ON GRAPH BANK EDGE TABLE IN_CITY").unwrap_err();
    assert!(e.msg.contains("already exists"), "named as the first statement named it: {e:?}");
    assert_eq!(indexes_of(&s, "labels").len(), 17);
    s.run("CREATE REACHABILITY INDEX R ON GRAPH BANK EDGE TABLE TRANSFER").unwrap();
    s.run("CREATE INDEX R2 ON TRANSFER (SRC, DST) USING reachability").unwrap();
    s.run("CREATE INDEX L2 ON PHONE (ID) USING labels").unwrap();
    assert_eq!(indexes_of(&s, "reachability"), ["R", "R2"]);
    let e = s.run("CREATE LABEL INDEX ON GRAPH NOPE").unwrap_err();
    assert!(e.sqlcode() == -103 && e.msg.contains("GRF-0001"), "{e:?}");
    let e = s.run("CREATE LABEL INDEX X ON GRAPH BANK EDGE TABLE ACCOUNT").unwrap_err();
    assert!(e.sqlcode() == -103 && e.msg.contains("GRF-0012"), "{e:?}");
    let e = s.run("CREATE REACHABILITY INDEX ON GRAPH BANK EDGE TABLE PHONE").unwrap_err();
    assert!(e.sqlcode() == -103 && e.msg.contains("GRF-0012"), "{e:?}");
    assert_eq!(s.run("CREATE REACHABILITY INDEX ON GRAPH BANK VERTEX TABLE ACCOUNT").unwrap_err().sqlcode(), -102);
    assert_eq!(s.run("CREATE INDEX R3 ON TRANSFER (SRC, AMOUNT) USING reachability").unwrap_err().sqlcode(), -900);
    assert_eq!(s.run("CREATE INDEX L3 ON ACCOUNT (OWNER) USING labels").unwrap_err().sqlcode(), -900);
}

/// Verifies: [EXTENSION] RULES.md D115, D116: a GRAPH_TABLE read through
/// label and reachability indexes gives the rows, or the error, it gives
/// without them, of every label operator and of pruned searches, after
/// edges are added (labels kept), taken back (labels dropped) and once
/// maintenance has made them again.
#[test]
fn indexed_searches_give_the_rows_unindexed_ones_do() {
    let (plain_db, kept_db) = (Database::new(), Database::new());
    let (plain, kept) = (fixture(&plain_db), fixture(&kept_db));
    indexed(&kept);
    same(&plain, &kept, "indexed");
    both(&plain, &kept, "INSERT INTO TRANSFER VALUES ('t9', 'a7', 'a4', DATE '2020-10-01', 5), ('t10', 'a5', 'a7', DATE '2020-10-02', 5)");
    same(&plain, &kept, "edges added");
    both(&plain, &kept, "DELETE FROM TRANSFER WHERE ID IN ('t3', 't8')");
    same(&plain, &kept, "edges taken back");
    kept.run("COMMIT WORK").unwrap();
    for ix in ["BANK_TRANSFER_REACHABILITY", "BANK_ACCOUNT_LABELS"] {
        kept.run(&format!("ALTER INDEX {ix} MERGE DELTA")).unwrap();
    }
    same(&plain, &kept, "maintained");
    both(&plain, &kept, "INSERT INTO ACCOUNT VALUES ('a8', 'Nell', 'no')");
    same(&plain, &kept, "a vertex added");
    let failing = "SELECT A FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a)-[t:Transfer]->+(b WHERE 1 / (CASE WHEN b.id = 'a6' THEN 0 ELSE 1 END) = 1) COLUMNS (a.id AS A))";
    assert_eq!(answers(&kept, &[failing]), answers(&plain, &[failing]));
    assert!(answers(&plain, &[failing])[0].is_err(), "the search reaches a6");
}

/// Verifies: [EXTENSION] RULES.md D115, D116: within a transaction a
/// GRAPH_TABLE read through the indexes sees its own changes, and not
/// those it rolled back.
#[test]
fn indexed_searches_see_their_own_changes() {
    let (plain_db, kept_db) = (Database::new(), Database::new());
    let (plain, kept) = (fixture(&plain_db), fixture(&kept_db));
    indexed(&kept);
    for s in [&plain, &kept] {
        s.run("INSERT INTO TRANSFER VALUES ('t9', 'a7', 'a5', DATE '2020-10-01', 5)").unwrap();
        s.run("INSERT INTO ACCOUNT VALUES ('a9', 'Nine', 'no')").unwrap();
    }
    same(&plain, &kept, "uncommitted");
    plain.run("ROLLBACK WORK").unwrap();
    kept.run("ROLLBACK WORK").unwrap();
    same(&plain, &kept, "rolled back");
}

/// Verifies: [EXTENSION] [GS §16 R16.9] RULES.md D115, D116: EXPLAIN
/// names the reachability index that prunes a path pattern's starts and
/// the label index each element table's labels are tested by; a pattern
/// whose end has no WHERE of its own is not pruned.
#[test]
fn explain_names_the_indexes_read() {
    let db = Database::new();
    let s = fixture(&db);
    indexed(&s);
    let plan = s.explain(SEARCHES[0], &Params::new()).unwrap();
    for shown in [
        "path 1: starts pruned by reachability index BANK_TRANSFER_REACHABILITY",
        "element table ACCOUNT: labels tested by label index BANK_ACCOUNT_LABELS",
    ] {
        assert!(plan.contains(shown), "{shown}: {plan}");
    }
    let plan = s.explain(SEARCHES[5], &Params::new()).unwrap();
    assert!(!plan.contains("pruned"), "{plan}");
}

/// The database in `dir`, opened of `files`.
fn opened(dir: &Path, crashed: bool) -> Database {
    let db = Database::new();
    let files = if crashed { after_crash() } else { os() };
    db.open_files(&files, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D25, D115, D116: a graph's label and
/// reachability indexes opened again, and after a crash, give the rows
/// they gave, committed changes found and an uncommitted one not.
#[test]
fn graph_indexes_are_reopened_and_recovered() {
    let scratch = Scratch::new("graph-table-indexes");
    let dir = scratch.path("db");
    let db = opened(&dir, true);
    let s = fixture(&db);
    indexed(&s);
    let before = answers(&s, SEARCHES);
    s.run("COMMIT WORK").unwrap();
    db.close().unwrap();
    drop((s, db));
    let db = opened(&dir, true);
    let s = db.session("A");
    assert_eq!(answers(&s, SEARCHES), before, "reopened");
    assert_eq!(indexes_of(&s, "reachability").len(), 1);
    db.checkpoint().unwrap();
    s.run("INSERT INTO TRANSFER VALUES ('t9', 'a7', 'a4', DATE '2020-10-01', 5)").unwrap();
    s.run("COMMIT WORK").unwrap();
    let changed = (answers(&s, SEARCHES), answers(&s, LABEL_TESTS));
    s.run("INSERT INTO TRANSFER VALUES ('t10', 'a5', 'a7', DATE '2020-10-01', 5)").unwrap();
    drop(db);
    let db = opened(&dir, true);
    let s = db.session("A");
    assert_eq!(indexes_of(&s, "labels").len(), 8);
    assert_eq!((answers(&s, SEARCHES), answers(&s, LABEL_TESTS)), changed, "recovered");
    s.run("DROP PROPERTY GRAPH BANK").unwrap();
    s.run(BANK).unwrap();
    s.run("COMMIT WORK").unwrap();
    assert_eq!(answers(&s, SEARCHES), changed.0, "dropped no index");
}

/// Verifies: [EXTENSION] RULES.md D49, D115, D116: a temporary table
/// takes indexes of both methods, kept in memory of each session's rows.
#[test]
fn temporary_tables_take_graph_indexes() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    s.run("CREATE GLOBAL TEMPORARY TABLE HOP (SRC INTEGER NOT NULL, DST INTEGER NOT NULL) ON COMMIT PRESERVE ROWS").unwrap();
    s.run("COMMIT WORK").unwrap();
    s.run("CREATE INDEX HOP_R ON HOP (SRC, DST) USING reachability").unwrap();
    s.run("CREATE INDEX HOP_L ON HOP (SRC) USING labels").unwrap();
    s.run("INSERT INTO HOP VALUES (1, 2), (2, 3)").unwrap();
    s.run("DELETE FROM HOP WHERE SRC = 1").unwrap();
    s.run("COMMIT WORK").unwrap();
    assert_eq!(rows(&s, "SELECT SRC FROM HOP"), ["2"]);
}

/// Verifies: [EXTENSION] [GS §20] RULES.md D116: the starts a
/// reachability index drops are not searched: a search to the account no
/// transfer reaches passes MAX_SEARCH_STATES from every account, 54061,
/// and not from the one start kept, which is its end.
#[test]
fn pruned_starts_are_not_searched() {
    let db = Database::new();
    let s = fixture(&db);
    let q = "SELECT A FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a:Account)-[t:Transfer]->*(b WHERE b.id = 'a7') COLUMNS (a.id AS A) \
             OPTIONS (MAX_SEARCH_STATES 4))";
    let e = s.open_cursor(q).and_then(|mut c| c.fetch().map(|_| ())).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "54061");
    s.run("ROLLBACK WORK").unwrap();
    indexed(&s);
    assert_eq!(rows(&s, q), ["a7"]);
}
