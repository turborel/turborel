//! Nearest ordering beyond ascending ST_DISTANCE with FETCH FIRST ONLY:
//! WITH TIES, OFFSET, further sort keys, filters few rows pass, farthest
//! first, and a nearest read for each outer row (D85, D135).

use super::populated;
use crate::common::rows;
use turborel::db::{Database, Session};
use turborel::params::Params;

/// Each query's rows, its plan holding a nearest read of SHAPES_G or not
/// as `nearest` says, then the same rows once the index is dropped.
fn indexed_then_scanned(db: &Database, a: &Session, queries: &[(&str, bool)]) -> Vec<Vec<String>> {
    let mut results = Vec::new();
    for (query, nearest) in queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert_eq!(plan.contains("NearestScan A.SHAPES using SHAPES_G"), *nearest, "{query}\n{plan}");
        results.push(rows(a, query).unwrap());
    }
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    for ((query, _), expected) in queries.iter().zip(&results) {
        assert!(!a.physical_plan(query, &Params::new()).unwrap().contains("SHAPES_G"));
        assert_eq!(&rows(a, query).unwrap(), expected, "{query}");
    }
    results
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR5] D85, D135: FETCH FIRST WITH
/// TIES reads the nearest candidates as ONLY does, keeping every row tied
/// with the last; OFFSET with FETCH, further sort keys and a filter few
/// rows pass, the search going on until enough do, read them too.
/// PERCENT needs the count of every row, so it reads the table. Dropping
/// the index gives the same rows.
#[test]
fn nearest_reads_serve_ties_offsets_keys_and_filters() {
    let db = populated();
    let a = db.session("A");
    let at = "ST_DISTANCE(G, ST_POINT(500.5, 500.5, 4326))";
    let ties = format!("SELECT ID FROM SHAPES ORDER BY {at} FETCH FIRST 1 ROW WITH TIES");
    let keyed = format!("SELECT ID FROM SHAPES ORDER BY {at}, ID DESC OFFSET 1 ROW FETCH FIRST 3 ROWS ONLY");
    let filtered =
        format!("SELECT ID FROM SHAPES WHERE CASE WHEN ID > 1500 THEN 1 ELSE 0 END > 0 ORDER BY {at}, ID FETCH FIRST 3 ROWS ONLY");
    let keyed_ties = format!("SELECT ID FROM SHAPES ORDER BY {at}, ID DESC FETCH FIRST 1 ROW WITH TIES");
    let percent = format!("SELECT ID FROM SHAPES ORDER BY {at}, ID FETCH FIRST 1 PERCENT ROWS ONLY");
    let queries =
        [(ties.as_str(), true), (keyed.as_str(), true), (filtered.as_str(), true), (keyed_ties.as_str(), true), (percent.as_str(), false)];
    let results = indexed_then_scanned(&db, &a, &queries);
    let mut peers = results[0].clone();
    peers.sort();
    assert_eq!(peers, ["500", "501"]);
    assert_eq!(results[1], ["500", "502", "499"]);
    assert_eq!(results[2], ["2002", "1501", "1502"]);
    assert_eq!(results[3], ["501"]);
    assert_eq!(results[4].len(), 21);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.10 GR3] D85, D135: ORDER BY
/// ST_DISTANCE DESC NULLS LAST with FETCH FIRST reads the R-tree farthest
/// first, by the upper bounds of the distances, with ties, a filter and
/// further keys; its NULL and empty rows come last. NULLS FIRST, which
/// would put them first, reads the table. Dropping the index gives the
/// same rows.
#[test]
fn farthest_first_reads_use_upper_bounds() {
    let db = populated();
    let a = db.session("A");
    let far = "ST_DISTANCE(G, ST_POINT(999.5, 999.5, 4326))";
    let first = format!("SELECT ID FROM SHAPES ORDER BY {far} DESC NULLS LAST, ID FETCH FIRST 3 ROWS ONLY");
    let ties = format!("SELECT ID FROM SHAPES ORDER BY {far} DESC NULLS LAST FETCH FIRST 1 ROW WITH TIES");
    let filtered = format!("SELECT ID FROM SHAPES WHERE ID BETWEEN 10 AND 1500 ORDER BY {far} DESC NULLS LAST, ID FETCH FIRST 2 ROWS ONLY");
    let tail = "SELECT ID FROM SHAPES WHERE CASE WHEN ID >= 1999 THEN 1 ELSE 0 END > 0 ORDER BY ST_DISTANCE(G, ST_POINT(5, 5, 4326)) DESC NULLS LAST, ID FETCH FIRST 4 ROWS ONLY";
    let nulls = format!("SELECT ID FROM SHAPES ORDER BY {far} DESC NULLS FIRST, ID FETCH FIRST 3 ROWS ONLY");
    let queries = [(first.as_str(), true), (ties.as_str(), true), (filtered.as_str(), true), (tail, true), (nulls.as_str(), false)];
    let results = indexed_then_scanned(&db, &a, &queries);
    assert_eq!(results[0], ["0", "1999", "1"]);
    let mut peers = results[1].clone();
    peers.sort();
    assert_eq!(peers, ["0", "1999"]);
    assert_eq!(results[2], ["10", "11"]);
    assert_eq!(results[3], ["1999", "2002", "2000", "2001"]);
    assert_eq!(results[4], ["2000", "2001", "0"]);
}

/// Verifies: [ISO/IEC 9075:1992 §7.11 GR2] D85, D135: a subquery ordered
/// by the distance to its outer row's geometry, with FETCH FIRST, is
/// bound to each outer row's value and so reads the R-tree for each, as a
/// scalar subquery and as the IN subquery giving each row's nearest few.
/// Dropping the index gives the same rows.
#[test]
fn nearest_reads_for_each_outer_row() {
    let db = populated();
    let a = db.session("A");
    a.run("CREATE TABLE PROBES (ID INTEGER, G GEOMETRY(4326))").unwrap();
    a.run("INSERT INTO PROBES VALUES (1, ST_POINT(10.2, 10.2, 4326)), (2, ST_POINT(700.6, 700.6, 4326)), (3, NULL)").unwrap();
    a.commit().unwrap();
    let scalar =
        "SELECT Q.ID, (SELECT P.ID FROM SHAPES P ORDER BY ST_DISTANCE(P.G, Q.G), P.ID FETCH FIRST 1 ROW ONLY) FROM PROBES Q ORDER BY Q.ID";
    let few = "SELECT Q.ID, P.ID FROM PROBES Q, SHAPES P WHERE P.ID IN (SELECT S.ID FROM SHAPES S ORDER BY ST_DISTANCE(S.G, Q.G), S.ID FETCH FIRST 2 ROWS ONLY) ORDER BY Q.ID, P.ID";
    let inner = "SELECT P.ID FROM SHAPES P ORDER BY ST_DISTANCE(P.G, ST_POINT(700.6, 700.6, 4326)), P.ID FETCH FIRST 1 ROW ONLY";
    let plan = a.physical_plan(inner, &Params::new()).unwrap();
    assert!(plan.contains("NearestScan A.SHAPES using SHAPES_G"), "{plan}");
    let indexed = [rows(&a, scalar).unwrap(), rows(&a, few).unwrap()];
    assert_eq!(indexed[0], ["1,10", "2,701", "3,0"]);
    assert_eq!(indexed[1], ["1,10", "1,2002", "2,700", "2,701", "3,0", "3,1"]);
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    assert_eq!([rows(&a, scalar).unwrap(), rows(&a, few).unwrap()], indexed);
}
