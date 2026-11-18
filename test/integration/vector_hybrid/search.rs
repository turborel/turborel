//! HYBRID_SEARCH (the vector design document §11.2, §11.3): each list read
//! through its part of the hybrid index, fused by RRF or by weighted
//! scores normalized min-max, weighted, of RRF_K; its columns, its forms,
//! its dynamic parameters and what it refuses.

use turborel::db::Session;
use turborel::error::Code;
use turborel::value::Value;

use super::fixture::{DOCUMENTS, documents, plan, read};
use crate::common::rows;

/// The query vector, the first document's embedding.
const VECTOR: &str = "TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32')";
/// The index every test makes.
const INDEX: &str = "CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY, EMB)";

/// HYBRID_SEARCH of DOCS_H for 'refund damaged' and `VECTOR`, of `args`
/// more, its columns all, as `SELECT *` gives them.
fn hybrid(args: &str) -> String {
    format!("SELECT * FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'refund damaged', QUERY_VECTOR = {VECTOR}, {args}) AS H")
}

/// A row of `hybrid`.
#[derive(Debug)]
struct Fused {
    id: usize,
    score: f64,
    vector: Option<(f64, f64)>,
    text: Option<(f64, f64)>,
    chunk: String,
}

/// `value` as a number, none if null.
fn number(value: &str) -> Option<f64> {
    (value != "NULL").then(|| value.parse().unwrap())
}

/// The rows of `sql`, a query `hybrid` writes.
fn fused(s: &Session, sql: &str) -> Vec<Fused> {
    let parse = |row: String| {
        let c: Vec<&str> = row.splitn(7, ',').collect();
        let pair = |score: &str, rank: &str| number(score).zip(number(rank));
        Fused {
            id: c[0].parse().unwrap(),
            score: number(c[1]).unwrap(),
            vector: pair(c[2], c[4]),
            text: pair(c[3], c[5]),
            chunk: c[6].into(),
        }
    };
    read(s, sql).into_iter().map(parse).collect()
}

/// The IDs of `rows`.
fn ids(rows: &[Fused]) -> Vec<usize> {
    rows.iter().map(|r| r.id).collect()
}

/// Whether `a` and `b` are equal but for rounding.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Verifies: [EXTENSION] vector_standard §11.2, §11.3, R11.1, R11.2: with
/// RRF, a row's SCORE is the sum over the lists holding it of 1 over 60
/// and its rank there; its VECTOR_SCORE is 1 minus its distance, its
/// TEXT_SCORE its RANK, each rank 1 for the best, a list not holding it
/// giving none; CHUNK_TEXT is its text; the rows come highest SCORE
/// first, ties by DOC_ID; EXPLAIN shows each list read from its part of
/// the index, the vector one approximately, joined.
#[test]
fn rrf_sums_the_reciprocal_ranks_11_3() {
    let (_db, s) = documents(Some(INDEX));
    let q = hybrid("TOP_N = 5, CANDIDATES = 5");
    let found = fused(&s, &q);
    assert_eq!(ids(&found), [1, 3, 4, 2, 5]);
    for row in &found {
        let part = |list: Option<(f64, f64)>| list.map_or(0.0, |(_, rank)| 1.0 / (60.0 + rank));
        assert!(near(row.score, part(row.vector) + part(row.text)), "{row:?}");
        assert_eq!(row.chunk, DOCUMENTS[row.id - 1].0);
        let distance = format!("SELECT 1E0 - VECTOR_DISTANCE(EMB, {VECTOR}, 'COSINE') FROM DOCS WHERE ID = {}", row.id);
        assert!(near(row.vector.unwrap().0, number(&read(&s, &distance)[0]).unwrap()), "{row:?}");
        let ranked = format!("SELECT RANK(BODY, 'IS ABOUT \"refund damaged\"') FROM DOCS WHERE ID = {}", row.id);
        let rank = number(&read(&s, &ranked)[0]).unwrap();
        assert_eq!(row.text.map(|(score, _)| score), (rank > 0.0).then_some(rank), "{row:?}");
        // Of the index's statistics, rare words weigh more than of none.
        let unindexed = format!("SELECT RANK(CAST(BODY AS VARCHAR(400)), 'IS ABOUT \"refund damaged\"') FROM DOCS WHERE ID = {}", row.id);
        assert!(rank == 0.0 || rank > number(&read(&s, &unindexed)[0]).unwrap(), "{row:?}");
    }
    let ranks =
        |list: fn(&Fused) -> Option<(f64, f64)>| found.iter().filter_map(|r| list(r).map(|(_, rank)| (r.id, rank))).collect::<Vec<_>>();
    assert_eq!(ranks(|r| r.vector), [(1, 1.0), (3, 5.0), (4, 4.0), (2, 2.0), (5, 3.0)]);
    assert_eq!(ranks(|r| r.text), [(1, 1.0), (3, 2.0), (4, 3.0)]);
    let shown = plan(&s, &q);
    assert!(shown.contains("FullOuterJoin"), "{shown}");
    assert!(shown.contains("NearestScan S.DOCS using DOCS_H [hybrid]: RANK("), "{shown}");
    assert!(shown.contains("NearestScan S.DOCS using DOCS_H [hybrid]: VECTOR_DISTANCE("), "{shown}");
    assert!(shown.contains("approximate COSINE"), "{shown}");
}

/// Verifies: [EXTENSION] vector_standard §11.2, §11.3, R11.1: TOP_N keeps
/// the first rows, each list TOP_N times 4 candidates by default;
/// VECTOR_WEIGHT and TEXT_WEIGHT weigh each list's part, a weight of 0
/// leaving the other list's order; RRF_K is the K of the sum.
#[test]
fn weights_and_k_shape_the_rrf_sum_11_3() {
    let (_db, s) = documents(Some(INDEX));
    assert_eq!(ids(&fused(&s, &hybrid("TOP_N = 3"))), [1, 3, 4]);
    assert_eq!(ids(&fused(&s, &hybrid("TOP_N = 3, TEXT_WEIGHT = 0"))), [1, 2, 5]);
    assert_eq!(ids(&fused(&s, &hybrid("TOP_N = 3, VECTOR_WEIGHT = 0E0"))), [1, 3, 4]);
    let shaped = fused(&s, &hybrid("TOP_N = 1, VECTOR_WEIGHT = 0.5, TEXT_WEIGHT = 2, RRF_K = 1"));
    assert_eq!(ids(&shaped), [1]);
    assert!(near(shaped[0].score, 0.5 / 2.0 + 2.0 / 2.0), "{shaped:?}");
    let shown = plan(&s, &hybrid("TOP_N = 3"));
    assert!(shown.contains("first=12"), "{shown}");
}

/// Verifies: [EXTENSION] vector_standard §11.3: WEIGHTED sums each list's
/// weight times the row's score in it normalized min-max over the list's
/// candidates, a vector score 1 minus the distance; a list whose
/// candidates all score alike gives each 1.
#[test]
fn weighted_fusion_normalizes_min_max_11_3() {
    let (_db, s) = documents(Some(INDEX));
    let found = fused(&s, &hybrid("TOP_N = 5, CANDIDATES = 5, FUSION = 'WEIGHTED', TEXT_WEIGHT = 0.5"));
    assert_eq!(ids(&found), [1, 2, 5, 4, 3]);
    let bounds = |list: fn(&Fused) -> Option<(f64, f64)>| {
        let scores: Vec<f64> = found.iter().filter_map(|r| list(r).map(|(score, _)| score)).collect();
        (scores.iter().copied().fold(f64::INFINITY, f64::min), scores.iter().copied().fold(f64::NEG_INFINITY, f64::max))
    };
    let (vector, text) = (bounds(|r| r.vector), bounds(|r| r.text));
    let norm = |score: f64, (lo, hi): (f64, f64)| (score - lo) / (hi - lo);
    for row in &found {
        let v = row.vector.map_or(0.0, |(score, _)| norm(score, vector));
        let t = row.text.map_or(0.0, |(score, _)| norm(score, text));
        assert!(near(row.score, v + 0.5 * t), "{row:?}");
    }
    let alike = format!(
        "SELECT DOC_ID, SCORE FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'gift', QUERY_VECTOR = {VECTOR}, TOP_N = 5, \
         CANDIDATES = 1, FUSION = 'WEIGHTED') AS H"
    );
    // Each list one candidate, which scores 1; the approximate one need not be the nearest.
    let alike = read(&s, &alike);
    assert!(alike.contains(&"4,1E0".to_owned()) && alike.len() == 2 && alike.iter().all(|r| r.ends_with(",1E0")), "{alike:?}");
    // Three candidates, the farthest nearer than 1: its score is 0, not 1 minus its distance.
    let three = fused(&s, &hybrid("TOP_N = 8, CANDIDATES = 3, FUSION = 'WEIGHTED', TEXT_WEIGHT = 0"));
    let farthest = three
        .iter()
        .filter_map(|r| r.vector.map(|(score, _)| (score, r.score)))
        .fold((f64::INFINITY, 0.0), |a, b| if b.0 < a.0 { b } else { a });
    assert!(farthest.0 > 0.0 && near(farthest.1, 0.0), "{three:?}");
}

/// Verifies: [EXTENSION] vector_standard §11.2: the standard's form, its
/// rows joined to the table by DOC_ID; a correlation name of none written
/// is HYBRID_SEARCH, and a derived column list renames its columns.
#[test]
fn the_forms_of_the_table_function_11_2() {
    let (_db, s) = documents(Some(INDEX));
    let joined = format!(
        "SELECT D.ID FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'refund damaged', QUERY_VECTOR = {VECTOR}, TOP_N = 2, \
         FUSION = 'RRF', VECTOR_WEIGHT = 0.7, TEXT_WEIGHT = 0.3) AS H JOIN DOCS D ON D.ID = H.DOC_ID ORDER BY H.SCORE DESC"
    );
    assert_eq!(read(&s, &joined), ["1", "4"]);
    let unnamed =
        format!("SELECT HYBRID_SEARCH.DOC_ID FROM HYBRID_SEARCH(QUERY_VECTOR = {VECTOR}, TOP_N = 1, INDEX = DOCS_H, QUERY_TEXT = 'gift')");
    assert_eq!(read(&s, &unnamed), ["4"]);
    let listed = format!(
        "SELECT H.K, H.C FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'gift', QUERY_VECTOR = {VECTOR}, TOP_N = 1) \
         AS H (K, S, VS, TS, VR, TR, C)"
    );
    assert_eq!(read(&s, &listed), ["4,refund of a gift card"]);
}

/// Verifies: [EXTENSION] vector_standard §11.2, [ISO/IEC 9075:1992 §17.6]:
/// QUERY_TEXT and QUERY_VECTOR may be dynamic parameters of a statement
/// prepared, numbered in the statement's order, before or after
/// HYBRID_SEARCH's own, whichever argument comes first.
#[test]
fn dynamic_parameters_keep_their_order() {
    let (_db, s) = documents(Some(INDEX));
    let vector_first = "SELECT H.DOC_ID FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_VECTOR = TO_VECTOR(CAST(? AS VARCHAR(40)), 4, 'FLOAT32'), \
                        QUERY_TEXT = ?, TOP_N = 3) AS H WHERE H.CHUNK_TEXT <> ? ORDER BY H.SCORE DESC, H.DOC_ID";
    let p = s.prepare(vector_first).unwrap();
    let values = [Value::Char("[1,0,0,0]".into()), Value::Char("refund damaged".into()), Value::Char(DOCUMENTS[2].0.into())];
    let mut c = s.open_prepared(&p, &values).unwrap();
    let mut found = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        found.push(row.iter().map(ToString::to_string).collect::<String>());
    }
    assert_eq!(found, ["1", "4"]);
    let text_first = "SELECT CAST(? AS VARCHAR(4)), H.DOC_ID FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = ?, TOP_N = 1, \
                      QUERY_VECTOR = TO_VECTOR(CAST(? AS VARCHAR(40)), 4, 'FLOAT32')) AS H";
    let p = s.prepare(text_first).unwrap();
    let values = [Value::Char("x".into()), Value::Char("gift".into()), Value::Char("[0,0,1,0]".into())];
    let row = s.open_prepared(&p, &values).unwrap().fetch().unwrap().unwrap();
    assert_eq!(row.iter().map(ToString::to_string).collect::<Vec<_>>(), ["x", "4"]);
}

/// Verifies: [EXTENSION] vector_standard §11.2: HYBRID_SEARCH names a
/// hybrid index, of a table of a primary key of one column, its DOC_ID;
/// INDEX, QUERY_TEXT and TOP_N once each, the others at most once; no
/// QUERY_VECTOR only of an index of a MODEL; FUSION is RRF or WEIGHTED, a
/// weight a number.
#[test]
fn what_hybrid_search_refuses_11_2() {
    let (_db, s) = documents(Some(INDEX));
    s.run("CREATE INDEX DOCS_F ON DOCS (BODY) USING fulltext").unwrap();
    s.run("CREATE TABLE NOKEY (BODY VARCHAR(40), EMB VECTOR(4))").unwrap();
    s.commit().unwrap();
    s.run("CREATE HYBRID VECTOR INDEX NOKEY_H ON NOKEY (BODY, EMB)").unwrap();
    let refused = |args: &str| {
        let q = format!("SELECT * FROM HYBRID_SEARCH({args}) AS H");
        rows(&s, &q).expect_err(&q)
    };
    let all = format!("QUERY_TEXT = 'a', QUERY_VECTOR = {VECTOR}, TOP_N = 1");
    assert_eq!(refused(&format!("INDEX = DOCS_F, {all}")), Code::SyntaxRule);
    assert_eq!(refused(&format!("INDEX = NONE_SUCH, {all}")), Code::SyntaxRule);
    assert_eq!(refused(&format!("INDEX = NOKEY_H, {all}")), Code::SyntaxRule);
    assert_eq!(refused(&format!("INDEX = DOCS_H, QUERY_TEXT = 'a', QUERY_VECTOR = {VECTOR}")), Code::SyntaxRule);
    assert_eq!(refused("INDEX = DOCS_H, QUERY_TEXT = 'a', TOP_N = 1"), Code::SyntaxRule);
    assert_eq!(refused(&format!("INDEX = DOCS_H, QUERY_VECTOR = {VECTOR}, TOP_N = 1")), Code::SyntaxRule);
    assert_eq!(refused(&format!("INDEX = DOCS_H, INDEX = DOCS_H, {all}")), Code::SyntaxRule);
    assert_eq!(refused(&format!("INDEX = DOCS_H, {all}, FUSION = 'MAX'")), Code::Syntax);
    assert_eq!(refused(&format!("INDEX = DOCS_H, {all}, TEXT_WEIGHT = 'heavy'")), Code::SyntaxRule);
    assert_eq!(refused(&format!("INDEX = DOCS_H, {all}, LIMIT = 3")), Code::Syntax);
}

/// Verifies: [EXTENSION] vector_standard §11.2, [ISO/IEC 13249-2:2002
/// (CD, SC32 N0640) §5.1.2]: CONTAINS of the hybrid index's text column
/// is answered by its text part, as of a full-text index, and so is the
/// standard's plain form, a full-text predicate with a vector ORDER BY,
/// its vector part ordering the rows; each gives the table scan's rows.
#[test]
fn contains_reads_the_text_part_11_2() {
    let (_db, s) = documents(Some(INDEX));
    let contains = "SELECT ID FROM DOCS WHERE CONTAINS(BODY, '\"refund\"') = 1 ORDER BY ID";
    let shown = plan(&s, contains);
    assert!(shown.contains("using DOCS_H [hybrid]"), "{shown}");
    assert_eq!(read(&s, contains), ["1", "4"]);
    let plain = format!(
        "SELECT ID FROM DOCS WHERE CONTAINS(BODY, '\"damaged\"') > 0 \
         ORDER BY VECTOR_DISTANCE(EMB, {VECTOR}, 'COSINE') FETCH APPROX FIRST 2 ROWS ONLY"
    );
    assert_eq!(read(&s, &plain), ["1", "3"]);
    assert_eq!(read(&s, &plain), read(&s, &plain.replace("APPROX", "EXACT")));
}
