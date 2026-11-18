//! Approximate nearest-neighbour search (RULES.md D94): FETCH APPROX
//! and FETCH EXACT, the planner's choice of an approximate index, the
//! executor's re-rank, widening and fallback, filter strategies, hints and
//! settings, through a fake approximate index method (`fixture.rs`), apart
//! from the vector engines. CREATE VECTOR INDEX and ALTER INDEX are
//! `vector_search/ddl.rs`.

mod ddl;
mod fixture;

use std::sync::atomic::Ordering;

use fixture::{DEFAULT_LIST, database, indexed, plan, session};
use turborel::error::Code;
use turborel::index::FilterHint;

use crate::common::rows;

/// The ten rows nearest 500.4 by TEST_DIST, as `fetch` asks, with `filter`.
fn nearest(fetch: &str, filter: &str) -> String {
    format!("SELECT ID FROM T {filter} ORDER BY TEST_DIST(X, 500.4E0) FETCH {fetch} FIRST 10 ROWS ONLY")
}

/// Verifies: [EXTENSION] vector_standard §9 R9.2: FETCH APPROX of a table
/// with no index is a request, not a guarantee: the query runs exact and
/// gives exactly FETCH EXACT's rows, with no nearest read planned.
#[test]
fn approx_without_an_index_is_exact_r9_2() {
    let (db, _) = database(2000);
    let s = session(&db);
    let approx = rows(&s, &nearest("APPROX", "")).unwrap();
    assert_eq!(approx, rows(&s, &nearest("EXACT", "")).unwrap());
    assert_eq!(approx.len(), 10);
    assert_eq!(approx.first().map(String::as_str), Some("500"));
    assert!(!plan(&s, &nearest("APPROX", "")).contains("NearestScan"));
}

/// Verifies: [EXTENSION] vector_standard §13 R13.1, R13.5, §9.1: an
/// approximate index is read for FETCH APPROX of a distance of its own
/// metric, EXPLAIN naming the metric, the filter strategy and the search's
/// parameters; never for FETCH EXACT, nor of neither while
/// VECTOR.APPROX_DEFAULT is OFF; and its rows, re-ranked by the exact
/// distance, are the exact ones when its candidates hold them.
#[test]
fn approx_reads_the_index_and_exact_ignores_it_r13_1() {
    let (db, log) = indexed(2000);
    let s = session(&db);
    let shown = plan(&s, &nearest("APPROX", ""));
    assert!(shown.contains("NearestScan S.T using T_X [fakeann]"), "{shown}");
    assert!(shown.contains("approximate TESTL1 post-filter, target accuracy=index"), "{shown}");
    let exact = rows(&s, &nearest("EXACT", "")).unwrap();
    assert_eq!(rows(&s, &nearest("APPROX", "")).unwrap(), exact);
    assert_eq!(log.asked().len(), 1, "one search, its candidates enough");
    for fetch in ["EXACT", ""] {
        assert!(!plan(&s, &nearest(fetch, "")).contains("NearestScan"), "{fetch}");
    }
    assert_eq!(log.asked(), []);
}

/// Verifies: [EXTENSION] vector_standard §13 R13.1, §2 L3: a distance of
/// another metric reads no approximate index, so runs exact (EXPLAIN
/// shows no nearest read); EUCLIDEAN and EUCLIDEAN_SQUARED serve each
/// other, any other name not.
#[test]
fn a_metric_not_the_index_s_runs_exact_r13_1() {
    let (db, _) = indexed(500);
    let s = session(&db);
    let other = "SELECT ID FROM T ORDER BY TEST_DIST(X, 5.0E0, 'COSINE') FETCH APPROX FIRST 3 ROWS ONLY";
    assert!(!plan(&s, other).contains("NearestScan"));
    assert_eq!(rows(&s, other).unwrap(), ["5", "4", "6"]);
    let own = "SELECT ID FROM T ORDER BY TEST_DIST(X, 5.0E0, 'testl1') FETCH APPROX FIRST 3 ROWS ONLY";
    assert!(plan(&s, own).contains("NearestScan"));
    assert!(turborel::index::same_metric("EUCLIDEAN", "euclidean_squared"));
    assert!(turborel::index::same_metric("L2_SQUARED", "EUCLIDEAN"));
    assert!(!turborel::index::same_metric("EUCLIDEAN", "COSINE"));
}

/// Verifies: [EXTENSION] vector_standard §9.2, R9.4: WITH TARGET ACCURACY
/// and its PARAMETERS reach the index, EFSEARCH and SEARCH_LIST as the
/// search list; the session's VECTOR.SEARCH_LIST and VECTOR.BEAM_WIDTH
/// hold when the query names none; VECTOR.APPROX_DEFAULT ON makes a FETCH
/// of neither APPROX nor EXACT approximate.
#[test]
fn accuracy_parameters_and_settings_reach_the_index_r9_4() {
    let (db, log) = indexed(500);
    let s = session(&db);
    let q = "SELECT ID FROM T ORDER BY TEST_DIST(X, 5.0E0) FETCH APPROX FIRST 3 ROWS ONLY";
    rows(&s, &format!("{q} WITH TARGET ACCURACY 80")).unwrap();
    let asked = log.asked();
    assert_eq!((asked[0].accuracy, asked[0].search_list, asked[0].first, asked[0].widen), (Some(80), None, 3, 1));
    let parameters = " WITH TARGET ACCURACY PARAMETERS (EFSEARCH 200, BEAM_WIDTH 4, NEIGHBOR PARTITION PROBES 10)";
    rows(&s, &format!("{q}{parameters}")).unwrap();
    let asked = log.asked();
    assert_eq!((asked[0].search_list, asked[0].beam_width, asked[0].probes, asked[0].accuracy), (Some(200), Some(4), Some(10), None));
    assert!(plan(&s, &format!("{q}{parameters}")).contains("search list=200, beam width=4, probes=10"));
    s.run("SET VECTOR.SEARCH_LIST = 30").unwrap();
    s.run("SET VECTOR.BEAM_WIDTH = 2").unwrap();
    rows(&s, q).unwrap();
    rows(&s, &format!("{q} WITH TARGET ACCURACY PARAMETERS (SEARCH_LIST 7)")).unwrap();
    let asked = log.asked();
    assert_eq!((asked[0].search_list, asked[0].beam_width), (Some(30), Some(2)));
    assert_eq!(asked[1].search_list, Some(7));
    let unstated = "SELECT ID FROM T ORDER BY TEST_DIST(X, 5.0E0) FETCH FIRST 3 ROWS ONLY";
    assert!(!plan(&s, unstated).contains("NearestScan"));
    s.run("SET VECTOR.APPROX_DEFAULT = 'ON'").unwrap();
    assert!(plan(&s, unstated).contains("NearestScan"));
    assert_eq!(rows(&s, unstated).unwrap(), ["5", "4", "6"]);
}

/// Verifies: [EXTENSION] vector_standard §9 R9.3, R9.2: a search whose
/// candidates meet the condition too seldom is asked again twice as wide,
/// four times; then the table is read exactly, giving the rows asked. An
/// index that declines is read past at once. VECTOR.APPROX_FALLBACK OFF
/// refuses instead (SQLCODE -900).
#[test]
fn a_short_search_widens_then_falls_back_r9_3() {
    let (db, log) = indexed(2000);
    let s = session(&db);
    let q = "SELECT ID FROM T WHERE C = 7 ORDER BY TEST_DIST(X, 500.0E0) \
             FETCH APPROX FIRST 2 ROWS ONLY HINT (VECTOR_POSTFILTER)";
    assert_eq!(rows(&s, q).unwrap(), ["7", "1007"]);
    let widened: Vec<u32> = log.asked().iter().map(|a| a.widen).collect();
    assert_eq!(widened, [1, 2, 4, 8, 16]);
    s.run("SET VECTOR.APPROX_FALLBACK = 'OFF'").unwrap();
    assert_eq!(rows(&s, q).unwrap_err(), Code::Unsupported);
    log.asked();
    // Wide enough at the second search: no fallback needed.
    let near = "SELECT ID FROM T WHERE ID <> 500 AND ID <> 501 ORDER BY TEST_DIST(X, 500.0E0) \
                FETCH APPROX FIRST 4 ROWS ONLY HINT (VECTOR_POSTFILTER)";
    assert_eq!(rows(&s, near).unwrap(), ["499", "498", "502", "497"]);
    assert_eq!(log.asked().iter().map(|a| a.widen).collect::<Vec<_>>(), [1, 2]);
    log.decline.store(true, Ordering::Relaxed);
    assert_eq!(rows(&s, &nearest("APPROX", "")).unwrap_err(), Code::Unsupported);
    s.run("SET VECTOR.APPROX_FALLBACK = 'ON'").unwrap();
    assert_eq!(rows(&s, &nearest("APPROX", "")).unwrap(), rows(&s, &nearest("EXACT", "")).unwrap());
    assert_eq!(log.asked().len(), 2, "a declining index is asked once a search");
}

/// Verifies: [EXTENSION] vector_standard §13 R13.2, §19: a condition
/// keeping 0.1%, 10% or 90% of the rows still gives the k rows asked,
/// FETCH EXACT's, whichever strategy reads them; each hint names its
/// strategy, the in-filter asking the condition of candidates as the index
/// searches, the pre-filter reading no approximate index.
#[test]
fn filters_of_any_selectivity_give_k_rows_r13_2() {
    let (db, log) = indexed(4000);
    let s = session(&db);
    let three = |filter: &str| format!("SELECT ID FROM T {filter} ORDER BY TEST_DIST(X, 500.4E0) FETCH APPROX FIRST 3 ROWS ONLY");
    for (filter, share) in [("WHERE C < 1", "0.1%"), ("WHERE C < 100", "10%"), ("WHERE C < 900", "90%")] {
        let exact = rows(&s, &three(filter).replace("APPROX", "EXACT")).unwrap();
        assert_eq!(exact.len(), 3, "{share}");
        for hint in ["", " HINT (VECTOR_PREFILTER)", " HINT (VECTOR_INFILTER)", " HINT (VECTOR_POSTFILTER)"] {
            assert_eq!(rows(&s, &format!("{}{hint}", three(filter))).unwrap(), exact, "{share}{hint}");
        }
    }
    log.asked();
    let infilter = format!("{} HINT (VECTOR_INFILTER)", nearest("APPROX", "WHERE C < 100"));
    assert!(plan(&s, &infilter).contains("in-filter"));
    rows(&s, &infilter).unwrap();
    assert_eq!(log.asked().first().map(|a| a.filter), Some(FilterHint::InFilter));
    assert!(*log.filtered.lock().unwrap() > 0, "the index asked the condition of its candidates");
    assert!(!plan(&s, &format!("{} HINT (VECTOR_PREFILTER)", nearest("APPROX", "WHERE C < 100"))).contains("NearestScan"));
    let post = format!("{} HINT (VECTOR_POSTFILTER)", nearest("APPROX", "WHERE C < 900"));
    rows(&s, &post).unwrap();
    assert!(matches!(log.asked().first().map(|a| a.filter), Some(FilterHint::PostFilter(_))));
}

/// Verifies: [EXTENSION] vector_standard R13.4: NO_VECTOR_INDEX(T) keeps
/// the table from its approximate index; VECTOR_INDEX(T T_X) names it;
/// VECTOR_WAIT_FRESH changes nothing, every index being changed as its
/// table is; a hint naming another table is no error.
#[test]
fn hints_choose_or_bar_the_index_r13_4() {
    let (db, _) = indexed(500);
    let s = session(&db);
    let q = "SELECT ID FROM T ORDER BY TEST_DIST(X, 5.0E0) FETCH APPROX FIRST 3 ROWS ONLY";
    assert!(!plan(&s, &format!("{q} HINT (NO_VECTOR_INDEX(T))")).contains("NearestScan"));
    assert!(plan(&s, &format!("{q} HINT (NO_VECTOR_INDEX(U), VECTOR_INDEX(T T_X), VECTOR_WAIT_FRESH)")).contains("using T_X"));
    assert_eq!(rows(&s, &format!("{q} HINT (NO_VECTOR_INDEX(T))")).unwrap(), ["5", "4", "6"]);
}

/// Verifies: [EXTENSION] vector_standard R9.5: rows of equal distance are
/// ordered by row, the first inserted first, and the rows kept at the
/// boundary are the first by row.
#[test]
fn ties_are_broken_by_row_r9_5() {
    let (db, _) = indexed(0);
    let s = session(&db);
    for id in [9, 3, 7, 1, 5] {
        s.run(&format!("INSERT INTO T VALUES ({id}, 10.0E0, 0)")).unwrap();
    }
    s.run("INSERT INTO T VALUES (0, 10.5E0, 0)").unwrap();
    s.commit().unwrap();
    let q = "SELECT ID FROM T ORDER BY TEST_DIST(X, 10.5E0) FETCH APPROX FIRST 4 ROWS ONLY";
    assert!(plan(&s, q).contains("NearestScan"));
    assert_eq!(rows(&s, q).unwrap(), ["0", "9", "3", "7"]);
}

/// Verifies: [EXTENSION] vector_standard §9.2: FETCH APPROX is of a count
/// of rows ONLY, so WITH TIES and PERCENT are refused (SQLCODE -103); a
/// target accuracy is of APPROX alone, from 1 to 100 (-103); a search
/// parameter is at least 1 (-103); and an unknown one is a syntax error.
#[test]
fn approx_refuses_ties_percent_and_stray_accuracy() {
    let (db, _) = indexed(50);
    let s = session(&db);
    let order = "SELECT ID FROM T ORDER BY TEST_DIST(X, 5.0E0)";
    for (fetch, code) in [
        ("FETCH APPROX FIRST 3 ROWS WITH TIES", Code::SyntaxRule),
        ("FETCH APPROX FIRST 3 PERCENT ROWS ONLY", Code::SyntaxRule),
        ("FETCH FIRST 3 ROWS ONLY WITH TARGET ACCURACY 80", Code::SyntaxRule),
        ("FETCH EXACT FIRST 3 ROWS ONLY WITH TARGET ACCURACY 80", Code::SyntaxRule),
        ("FETCH APPROX FIRST 3 ROWS ONLY WITH TARGET ACCURACY 0", Code::SyntaxRule),
        ("FETCH APPROX FIRST 3 ROWS ONLY WITH TARGET ACCURACY 101", Code::SyntaxRule),
        ("FETCH APPROX FIRST 3 ROWS ONLY WITH TARGET ACCURACY PARAMETERS (EFSEARCH 0)", Code::SyntaxRule),
        ("FETCH APPROX FIRST 3 ROWS ONLY WITH TARGET ACCURACY PARAMETERS (NPROBE 3)", Code::Syntax),
        ("FETCH APPROX FIRST 3 ROWS ONLY HINT (VECTOR_FASTEST)", Code::Syntax),
    ] {
        assert_eq!(rows(&s, &format!("{order} {fetch}")).unwrap_err(), code, "{fetch}");
    }
    assert_eq!(rows(&s, &format!("{order} FETCH APPROXIMATE NEXT 2 ROWS ONLY WITH TARGET ACCURACY 100")).unwrap(), ["5", "4"]);
    assert_eq!(rows(&s, &format!("{order} FETCH EXACT FIRST 3 ROWS WITH TIES")).unwrap(), ["5", "4", "6"]);
}

/// Verifies: [EXTENSION] vector_standard §9.4 R9.1: a range search, a
/// condition that the distance is below a bound, tells the index the
/// radius, and gives the rows within it, nearest first.
#[test]
fn a_range_search_tells_the_index_its_radius_r9_1() {
    let (db, log) = indexed(500);
    let s = session(&db);
    let q = "SELECT ID FROM T WHERE TEST_DIST(X, 100.0E0) < 2.5E0 ORDER BY TEST_DIST(X, 100.0E0) FETCH APPROX FIRST 5 ROWS ONLY \
             HINT (VECTOR_POSTFILTER)";
    assert_eq!(rows(&s, q).unwrap(), ["100", "99", "101", "98", "102"]);
    assert_eq!(log.asked().first().and_then(|a| a.radius), Some(2.5), "{}", plan(&s, q));
}

/// Verifies: [EXTENSION] vector_standard §9.5: a batch search, a query
/// vector of each row of a driving table, here in a subquery with FETCH
/// APPROX inside, reads the index for each of its rows.
#[test]
fn a_batch_search_reads_the_index_for_each_row_9_5() {
    let (db, log) = indexed(500);
    let s = session(&db);
    s.run("CREATE TABLE Q (QX DOUBLE PRECISION)").unwrap();
    s.run("INSERT INTO Q VALUES (10.2E0), (299.9E0)").unwrap();
    s.commit().unwrap();
    let q = "SELECT (SELECT ID FROM T ORDER BY TEST_DIST(X, Q.QX) FETCH APPROX FIRST 1 ROW ONLY) FROM Q ORDER BY QX";
    assert_eq!(rows(&s, q).unwrap(), ["10", "300"]);
    assert_eq!(log.asked().len(), 2);
}

/// Verifies: [EXTENSION] vector_standard R10.6, R9.3: rows of a null
/// distance are never an approximate search's; asked for more rows than
/// have one, the search falls back, giving the nulls last.
#[test]
fn null_distances_come_last_r10_6() {
    let (db, _) = indexed(3);
    let s = session(&db);
    s.run("INSERT INTO T VALUES (99, NULL, 0)").unwrap();
    s.commit().unwrap();
    let q = "SELECT ID FROM T ORDER BY TEST_DIST(X, 0.0E0) FETCH APPROX FIRST 4 ROWS ONLY";
    assert_eq!(rows(&s, q).unwrap(), ["0", "1", "2", "99"]);
    assert_eq!(DEFAULT_LIST, 4);
}
