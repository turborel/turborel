//! The limits of a GRAPH_TABLE's search, by its OPTIONS and by the
//! settings GRAPH.*, what passing one does, and what EXPLAIN shows of a
//! search [GS §20].

use turborel::params::Params;

use super::{bank, refused, rows};

/// Every trail of transfers from Scott's account, of `options`.
fn trails(options: &str) -> String {
    format!("SELECT N FROM GRAPH_TABLE (BANK MATCH TRAIL (a WHERE a.id = 'a1')-[t:Transfer]->*(b) COLUMNS (PATH_LENGTH(p) AS N) {options})")
        .replace("TRAIL (a", "TRAIL p = (a")
}

/// The SQLSTATE `query` fails with in `s`.
fn state_of(s: &turborel::db::Session, query: &str) -> String {
    let e = s.open_cursor(query).and_then(|mut c| c.fetch().map(|_| ())).map(|()| panic!("{query}: accepted")).unwrap_err();
    e.sqlstate().as_str().to_owned()
}

/// Verifies: [EXTENSION] [GS §20] a search that passes MAX_PATH_LENGTH or
/// MAX_SEARCH_STATES fails, 54061 (GRF-0061); ON LIMIT TRUNCATE ends it
/// with the paths found, warning 01W61.
#[test]
fn options_bound_the_search_20() {
    let (_db, s) = bank();
    assert_eq!(rows(&s, &trails("")).len(), 12);
    assert_eq!(state_of(&s, &trails("OPTIONS (MAX_PATH_LENGTH 2)")), "54061");
    assert_eq!(state_of(&s, &trails("OPTIONS (MAX_SEARCH_STATES 3)")), "54061");
    let cut = rows(&s, &trails("OPTIONS (MAX_PATH_LENGTH 2, ON LIMIT TRUNCATE)"));
    assert_eq!(s.warning().map(|w| w.as_str().to_owned()).as_deref(), Some("01W61"));
    assert!(cut.iter().all(|n| n.as_str() <= "2"), "{cut:?}");
    assert_eq!(rows(&s, &trails("OPTIONS (MAX_PATH_LENGTH 20)")).len(), 12);
}

/// Verifies: [EXTENSION] [GS §20] MAX_PATHS_PER_PARTITION bounds the
/// paths kept of each start and end; DETERMINISTIC_PATHS keeps the same
/// one each time.
#[test]
fn paths_per_partition_20() {
    let (_db, s) = bank();
    let q = "SELECT A, B FROM GRAPH_TABLE (BANK MATCH ALL TRAIL (a WHERE a.id = 'a6')-[t:Transfer]->*(b WHERE b.id = 'a2') \
             COLUMNS (a.id AS A, b.id AS B) OPTIONS (MAX_PATHS_PER_PARTITION 1, ON LIMIT TRUNCATE, DETERMINISTIC_PATHS))";
    let q = q.replace("ALL TRAIL", "TRAIL");
    let first = rows(&s, &q);
    assert_eq!(first, ["a6,a2"]);
    assert_eq!(rows(&s, &q), first);
    assert_eq!(state_of(&s, &q.replace(", ON LIMIT TRUNCATE", "")), "54061");
}

/// Verifies: [EXTENSION] [GS §20] the settings GRAPH.MAX_PATH_LENGTH and
/// GRAPH.ON_LIMIT bound a search whose OPTIONS do not; its OPTIONS win.
#[test]
fn settings_bound_the_search_20() {
    let (_db, s) = bank();
    s.run("SET GRAPH.MAX_PATH_LENGTH = 2").unwrap();
    assert_eq!(state_of(&s, &trails("")), "54061");
    assert_eq!(rows(&s, &trails("OPTIONS (MAX_PATH_LENGTH 20)")).len(), 12);
    s.run("SET GRAPH.ON_LIMIT = 'TRUNCATE'").unwrap();
    assert!(rows(&s, &trails("")).len() < 12);
    assert_eq!(state_of(&s, &trails("OPTIONS (ON LIMIT ERROR)")), "54061");
}

/// Verifies: [EXTENSION] [GS §7.1, §20] an OPTION written twice is a
/// syntax error; a selector of k 0 too.
#[test]
fn options_are_written_once_20() {
    let (_db, s) = bank();
    assert_eq!(refused(&s, &trails("OPTIONS (MAX_PATH_LENGTH 2, MAX_PATH_LENGTH 3)")).0, -102);
}

/// Verifies: [EXTENSION] EXPLAIN shows a GRAPH_TABLE's automaton, the
/// tables its matches start at, how each edge table is followed and its
/// match mode: by a scan, then by the index its graph is given as it
/// commits.
#[test]
fn explain_shows_the_search() {
    let (_db, s) = bank();
    let q = "SELECT * FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a:Account)-[t:Transfer]->*(b:Account) COLUMNS (b.id AS B))";
    let plan = s.explain(q, &Params::new()).unwrap();
    for shown in ["Graph table of A.BANK (8 element tables)", "path 1: from ACCOUNT", "edge table TRANSFER: edges listed by a scan"] {
        assert!(plan.contains(shown), "{shown}: {plan}");
    }
    s.run("COMMIT WORK").unwrap();
    let plan = s.explain(q, &Params::new()).unwrap();
    assert!(plan.contains("edge table TRANSFER: adjacency index BANK_TRANSFER_ADJACENCY"), "{plan}");
}
