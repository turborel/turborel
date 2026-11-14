//! Row pattern recognition in FROM [ISO/IEC 9075-2:2023 §6.27, §6.31,
//! §6.33, §7.6, §7.7 to §7.9, §9.41, §10.9, Features R010, R030]: the
//! matches of MATCH_RECOGNIZE, the rows each gives, its measures, where the
//! next match is sought, and what its rules refuse.

use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{code, exec, expected, prices, recognized, rows, state};

/// The V of a start, prices falling, then rising.
const V: &str = "STRT DOWN+ UP+";

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 SR7, SR9, GR5, GR6, GR8b
/// ii, GR11a, §9.41 GR10, Feature R010] ONE ROW PER MATCH gives the
/// partitioning columns then the measures of each match, sought from each
/// partition's first row and after each past its last row, greedy
/// quantifiers taking the most rows; MATCH_NUMBER counts the matches of
/// each partition.
#[test]
fn one_row_per_match_gives_each_matchs_measures_7_7() {
    let (_db, a) = prices();
    let measures = "STRT.D AS S, LAST(DOWN.D) AS B, LAST(UP.D) AS E, MATCH_NUMBER() AS M";
    let q = format!("{} ORDER BY TICKER, S", recognized(measures, "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V));
    assert_eq!(rows(&a, &q), expected(&["A,1,3,6,1", "A,7,8,9,2", "B,1,2,3,1"]));
    // [§7.7 SR5, §7.9 SR23a] ONE ROW PER MATCH and SKIP PAST LAST ROW
    // implicit.
    let implicit = format!(
        "SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES {measures} PATTERN ({V}) \
         DEFINE DOWN AS PRICE < PREV(PRICE), UP AS PRICE > PREV(PRICE)) ORDER BY TICKER, S"
    );
    assert_eq!(rows(&a, &implicit), rows(&a, &q));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 SR8, SR10, GR11b, §6.33,
/// §4.23.2, Feature R010] ALL ROWS PER MATCH gives each row of each match,
/// the partitioning columns, the ordering's, the measures then the input's
/// other columns; CLASSIFIER names each row's variable; a measure is
/// RUNNING, of the rows up to the row given, unless FINAL.
#[test]
fn all_rows_per_match_gives_each_rows_running_measures_7_7() {
    let (_db, a) = prices();
    let measures =
        "CLASSIFIER() AS C, RUNNING COUNT(*) AS RC, FINAL COUNT(*) AS FC, MATCH_NUMBER() AS M, FINAL LAST(PRICE) AS FP, PRICE AS RP";
    let q = format!("{} WHERE TICKER = 'A' ORDER BY D", recognized(measures, "ALL ROWS PER MATCH", "SKIP PAST LAST ROW", V));
    assert_eq!(
        rows(&a, &q),
        expected(&[
            "A,1,STRT,1,6,1,11,10,10",
            "A,2,DOWN,2,6,1,11,9,9",
            "A,3,DOWN,3,6,1,11,8,8",
            "A,4,UP,4,6,1,11,9,9",
            "A,5,UP,5,6,1,11,10,10",
            "A,6,UP,6,6,1,11,11,11",
            "A,7,STRT,1,3,2,12,10,10",
            "A,8,DOWN,2,3,2,12,9,9",
            "A,9,UP,3,3,2,12,12,12",
        ])
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR8b, GR11b ii 1, GR11b
/// iii, NOTE 232, NOTE 233, §7.9 SR17] an empty match gives, of SHOW EMPTY
/// MATCHES, its row with measures of no rows: COUNT 0, a navigation and
/// CLASSIFIER null, MATCH_NUMBER its number; none of OMIT EMPTY MATCHES;
/// WITH UNMATCHED ROWS gives each row no match maps, its measures null, and
/// not a row an earlier match maps; a variable of no definition is TRUE.
#[test]
fn empty_matches_and_unmatched_rows_7_7() {
    let (_db, a) = prices();
    let empty = |rows: &str| {
        format!(
            "SELECT D, N, L, C, M FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES COUNT(*) AS N, LAST(PRICE) AS L, \
             CLASSIFIER() AS C, MATCH_NUMBER() AS M {rows} PATTERN (X*) DEFINE X AS PRICE > 100) WHERE TICKER = 'B' ORDER BY D"
        )
    };
    let shown = expected(&["1,0,NULL,NULL,1", "2,0,NULL,NULL,2", "3,0,NULL,NULL,3"]);
    assert_eq!(rows(&a, &empty("ALL ROWS PER MATCH")), shown);
    assert_eq!(rows(&a, &empty("ALL ROWS PER MATCH SHOW EMPTY MATCHES")), shown);
    assert_eq!(rows(&a, &empty("ALL ROWS PER MATCH OMIT EMPTY MATCHES")), expected(&[]));
    assert_eq!(rows(&a, &empty("ALL ROWS PER MATCH WITH UNMATCHED ROWS")), shown);
    let unmatched = "SELECT D, N, M FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES COUNT(*) AS N, MATCH_NUMBER() AS M \
                     ALL ROWS PER MATCH WITH UNMATCHED ROWS PATTERN (X Y) DEFINE X AS PRICE = 4) WHERE TICKER = 'B' ORDER BY D";
    assert_eq!(rows(&a, unmatched), expected(&["1,NULL,NULL", "2,1,1", "3,2,1"]));
    // A row an earlier match maps, though no match starts at it, is no
    // unmatched row.
    let next = unmatched.replace("WITH UNMATCHED ROWS", "WITH UNMATCHED ROWS AFTER MATCH SKIP TO NEXT ROW");
    assert_eq!(rows(&a, &next), expected(&["1,NULL,NULL", "2,1,1", "3,2,1"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR8b, §7.9 SR23b] where
/// the next match is sought: from the row after a match's first of SKIP TO
/// NEXT ROW, overlapping it; from the last or first row mapped to a
/// variable of SKIP TO LAST and FIRST, SKIP TO meaning LAST; of a union
/// variable, any of its components' rows.
#[test]
fn where_the_next_match_is_sought_7_7() {
    let (_db, a) = prices();
    let starts = |skip: &str| {
        let q = format!("{} WHERE TICKER = 'A' ORDER BY S", recognized("STRT.D AS S", "ONE ROW PER MATCH", skip, V));
        rows(&a, &q).unwrap()
    };
    assert_eq!(starts("SKIP TO NEXT ROW"), ["A,1", "A,2", "A,6", "A,7"]);
    assert_eq!(starts("SKIP PAST LAST ROW"), ["A,1", "A,7"]);
    assert_eq!(starts("SKIP TO LAST UP"), ["A,1", "A,6"]);
    assert_eq!(starts("SKIP TO UP"), ["A,1", "A,6"]);
    assert_eq!(starts("SKIP TO FIRST UP"), ["A,1", "A,6"]);
    assert_eq!(starts("SKIP TO LAST DOWN"), ["A,1", "A,6"]);
    assert_eq!(starts("SKIP TO DOWN"), ["A,1", "A,6"], "SKIP TO is SKIP TO LAST");
    assert_eq!(starts("SKIP TO FIRST DOWN"), ["A,1", "A,2", "A,6", "A,7"]);
    let union = "SELECT S FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES STRT.D AS S AFTER MATCH SKIP TO FIRST DU \
                 PATTERN (STRT DOWN+ UP+) SUBSET DU = (DOWN, UP) DEFINE DOWN AS PRICE < PREV(PRICE), UP AS PRICE > PREV(PRICE)) \
                 WHERE TICKER = 'A' ORDER BY S";
    assert_eq!(rows(&a, union), expected(&["1", "2", "6", "7"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR8b iii 1, 2A, 3A] a
/// skip to a variable no row of the match is mapped to is SQLSTATE 2202K;
/// to the match's first row, 2202L.
#[test]
fn a_skip_to_no_row_or_the_first_raises_7_7() {
    let (_db, a) = prices();
    let skipping = |skip: &str, pattern: &str| state(&a, &recognized("STRT.D AS S", "ONE ROW PER MATCH", skip, pattern));
    assert_eq!(skipping("SKIP TO X", "STRT DOWN+ UP+ X?"), "2202K");
    assert_eq!(skipping("SKIP TO FIRST X", "STRT DOWN+ UP+ X?"), "2202K");
    assert_eq!(skipping("SKIP TO STRT", V), "2202L");
    assert_eq!(skipping("SKIP TO FIRST STRT", V), "2202L");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 SR5 to SR8, GR2, GR11]
/// PREV and NEXT move by the physical offset, 1 if none, from the row FIRST
/// or LAST picks by the logical offset, 0 if none, among a variable's rows;
/// null where there is no such row; a negative offset is SQLSTATE 2202J, a
/// negative literal SQLCODE -103.
#[test]
fn navigation_picks_a_row_and_moves_6_27() {
    let (_db, a) = prices();
    let measures = "FIRST(DOWN.PRICE) AS FD, LAST(DOWN.PRICE, 1) AS LD1, PREV(STRT.PRICE) AS PS, NEXT(LAST(UP.PRICE)) AS NU, \
                    PREV(FIRST(UP.PRICE), 2) AS X, FIRST(UP.PRICE, 1) AS U1";
    let q = format!("{} WHERE TICKER = 'A' ORDER BY FD, X", recognized(measures, "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V));
    assert_eq!(rows(&a, &q), expected(&["A,9,9,NULL,10,9,10", "A,9,NULL,11,NULL,10,NULL"]));
    let negative = recognized("PREV(PRICE, -1) AS P", "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V);
    assert_eq!(code(&a, &negative), Code::SyntaxRule, "SR6a");
    let q = a.prepare(&recognized("PREV(PRICE, ?) AS P", "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V)).unwrap();
    let failed = a.open_prepared(&q, &[Value::Exact(Decimal::new(-1, 0))]).and_then(|mut o| o.fetch()).err();
    assert_eq!(failed.map(|e| e.sqlstate().as_str().to_owned()).as_deref(), Some("2202J"));
    let mut opened = a.open_prepared(&q, &[Value::Exact(Decimal::new(2, 0))]).unwrap();
    assert_eq!(opened.fetch().unwrap().map(|r| r[1].to_string()).as_deref(), Some("9"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR5, SR6, SR15, GR1b,
/// GR1c, Feature R030] a set function over the rows mapped to the one
/// variable its argument reads, the universal one if none, COUNT(V.*)
/// counting them; DISTINCT and FILTER; in a definition, of the rows up to
/// the candidate.
#[test]
fn set_functions_over_a_variables_rows_10_9() {
    let (_db, a) = prices();
    let measures = "SUM(DOWN.PRICE) AS SD, MIN(UP.PRICE) AS MU, MAX(UP.PRICE) AS XU, COUNT(DOWN.*) AS CD, COUNT(DISTINCT PRICE) AS DP, \
                    SUM(PRICE) FILTER (WHERE PRICE > 9) AS F, LISTAGG(CLASSIFIER(), '') WITHIN GROUP (ORDER BY D) AS L";
    let q = format!("{} WHERE TICKER = 'A' ORDER BY SD DESC", recognized(measures, "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V));
    assert_eq!(rows(&a, &q), expected(&["A,17,9,11,2,4,31,STRTDOWNDOWNUPUPUP", "A,9,12,12,1,3,22,STRTDOWNUP"]));
    let runs = "SELECT S, E FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES FIRST(D) AS S, LAST(D) AS E \
                PATTERN (X+) DEFINE X AS SUM(PRICE) < 30) WHERE TICKER = 'A' ORDER BY S";
    assert_eq!(rows(&a, runs), expected(&["1,3", "4,5", "6,7", "8,9"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6, GR10, §7.9 SR19,
/// SR21] the preferred match: a reluctant quantifier takes the fewest rows;
/// `^` and `$` match at a partition's ends alone; PERMUTE tries its orders
/// lexicographically; an alternation prefers its left.
#[test]
fn the_preferred_match_9_41() {
    let (_db, a) = prices();
    let ends = |pattern: &str, skip: &str| {
        let q = format!("{} WHERE TICKER = 'A' ORDER BY S", recognized("STRT.D AS S, LAST(D) AS E", "ONE ROW PER MATCH", skip, pattern));
        rows(&a, &q).unwrap()
    };
    assert_eq!(ends("STRT DOWN+? UP+?", "SKIP PAST LAST ROW"), ["A,1,4", "A,6,9"]);
    assert_eq!(ends("^ STRT DOWN+ UP+", "SKIP TO NEXT ROW"), ["A,1,6"]);
    assert_eq!(ends("STRT DOWN+ UP+ $", "SKIP TO NEXT ROW"), ["A,6,9", "A,7,9"]);
    let permuted = "SELECT C FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES LISTAGG(CLASSIFIER(), ',') WITHIN GROUP (ORDER BY D) AS C \
                    PATTERN (PERMUTE(P, Q)) DEFINE P AS PRICE >= 9, Q AS PRICE <= 9) WHERE TICKER = 'A' ORDER BY C";
    assert_eq!(rows(&a, permuted).unwrap().first().map(String::as_str), Some("P,Q"));
    let alternated = "SELECT C FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES CLASSIFIER() AS C \
                      PATTERN (Q | P) DEFINE P AS PRICE >= 9, Q AS PRICE <= 9) WHERE TICKER = 'A' ORDER BY C";
    assert_eq!(rows(&a, alternated), expected(&["P", "P", "P", "P", "P", "Q", "Q", "Q", "Q"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR11b ii 2 B I, §7.9
/// SR22, NOTE 234] rows mapped within `{- -}` give no row of ALL ROWS PER
/// MATCH, but are of the match's measures.
#[test]
fn excluded_rows_give_no_row_7_7() {
    let (_db, a) = prices();
    let q = format!(
        "{} WHERE TICKER = 'A' ORDER BY D",
        recognized("FINAL COUNT(*) AS N", "ALL ROWS PER MATCH", "SKIP PAST LAST ROW", "STRT {- DOWN+ -} UP+")
    );
    assert_eq!(
        rows(&a, &q).unwrap().iter().map(|r| r.split(',').nth(1).unwrap_or_default().to_owned()).collect::<Vec<_>>(),
        ["1", "4", "5", "6", "7", "9"]
    );
    let unmatched = q.replace("ALL ROWS PER MATCH", "ALL ROWS PER MATCH WITH UNMATCHED ROWS");
    assert_eq!(code(&a, &unmatched), Code::SyntaxRule, "SR22");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6 SR8b i 3 to 7] the row
/// pattern input name and derived column list, seen by PARTITION BY and
/// ORDER BY alone; the output name and derived column list; a view of
/// MATCH_RECOGNIZE.
#[test]
fn input_and_output_names_7_6() {
    let (_db, a) = prices();
    let q = "SELECT Z.K, Z.N FROM T AS I (K, D, P) MATCH_RECOGNIZE (PARTITION BY I.K ORDER BY I.D MEASURES COUNT(*) AS N PATTERN (X+) \
             DEFINE X AS P > 0) AS Z (K, N) ORDER BY Z.K";
    assert_eq!(rows(&a, q), expected(&["A,9", "B,3"]));
    let input_in_define = q.replace("DEFINE X AS P > 0", "DEFINE X AS I.P > 0");
    assert_eq!(code(&a, &input_in_define), Code::SyntaxRule, "§7.6 SR8b i 5");
    exec(&a, &format!("CREATE VIEW VS AS {}", recognized("STRT.D AS S, LAST(UP.D) AS E", "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V)))
        .unwrap();
    assert_eq!(rows(&a, "SELECT TICKER, S, E FROM VS ORDER BY TICKER, S"), expected(&["A,1,6", "A,7,9", "B,1,3"]));
    assert_eq!(exec(&a, "DELETE FROM VS").err(), Some(Code::SyntaxRule), "a view of MATCH_RECOGNIZE is not updatable");
    let two = "SELECT COUNT(*) FROM T MATCH_RECOGNIZE (PARTITION BY TICKER PATTERN (X) DEFINE X AS TRUE), \
               T MATCH_RECOGNIZE (PARTITION BY TICKER PATTERN (X) DEFINE X AS TRUE)";
    assert_eq!(rows(&a, two), expected(&["144"]), "two implicit output names are distinct (UV081)");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 SR1 to SR5, §6.31
/// SR22a, §6.33 SR24a, §6.9 SR11, SR12, SR13, §7.7 SR3, SR4, SR11, §7.8
/// SR3, §7.9 SR3a, SR9 to SR15, SR20, §10.9 SR6, SR15, §7.17 SR3f] what the
/// Syntax Rules refuse, each SQLCODE -103.
#[test]
fn the_syntax_rules_refuse_7_9() {
    let (_db, a) = prices();
    let refused = |sql: &str, why: &str| assert_eq!(code(&a, sql), Code::SyntaxRule, "{why}: {sql}");
    let one = |measures: &str| recognized(measures, "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V);
    refused(&one("PREV(DOWN.PRICE + UP.PRICE) AS X"), "§6.27 SR4");
    refused(&one("PREV(PREV(PRICE)) AS X"), "§6.27 SR2");
    refused(&one("PREV(SUM(PRICE)) AS X"), "§6.27 SR2");
    refused(&one("PREV(1) AS X"), "§6.27 SR3");
    refused(&one("SUM(PREV(PRICE)) AS X"), "§6.9 SR13");
    refused(&one("SUM(DOWN.PRICE + UP.PRICE) AS X"), "§10.9 SR15");
    refused(&one("GROUPING(TICKER) AS X"), "§6.9 SR11a i");
    refused(&one("(SELECT MAX(D) FROM T WHERE D < STRT.D) AS X"), "§7.8 SR3");
    refused(&one("COUNT(Z.*) AS X"), "§7.9 SR5");
    refused("SELECT PREV(PRICE) FROM T", "§6.27 SR1");
    refused("SELECT MATCH_NUMBER() FROM T", "§6.31 SR22a");
    refused("SELECT CLASSIFIER() FROM T", "§6.33 SR24a");
    refused("SELECT RUNNING COUNT(*) FROM T", "§6.9 SR11b");
    refused("SELECT COUNT(T.*) FROM T", "§10.9 SR6");
    let defined = |define: &str| {
        format!("SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES COUNT(*) AS N PATTERN (A B*) {define})")
    };
    refused(&defined("DEFINE A AS FINAL LAST(PRICE) > 0"), "§6.27 SR5b");
    refused(&defined("DEFINE A AS FINAL COUNT(*) > 0"), "§6.9 SR12");
    refused(&defined("DEFINE C AS TRUE"), "§7.9 SR10");
    refused(&defined("DEFINE A AS TRUE, A AS TRUE"), "§7.9 SR12");
    refused(&defined("SUBSET A = (B) DEFINE B AS TRUE"), "§7.9 SR9");
    refused(&defined("SUBSET U = (C) DEFINE B AS TRUE"), "§7.9 SR11");
    refused(&defined("SUBSET U = (A), U = (B) DEFINE B AS TRUE"), "§7.9 SR15");
    refused(&defined("DEFINE A AS PRICE > (SELECT MAX(PRICE) FROM T X WHERE X.D = B.D)"), "§7.9 SR14");
    refused(
        "SELECT (SELECT COUNT(*) FROM T MATCH_RECOGNIZE (PARTITION BY TICKER PATTERN (A) DEFINE A AS PRICE > O.PRICE)) FROM T O",
        "§7.9 SR13",
    );
    refused(&defined("DEFINE A AS TRUE").replace("PATTERN", "INITIAL PATTERN"), "§7.9 SR3a");
    refused(&defined("DEFINE A AS TRUE").replace("(A B*)", "(A{3,2})"), "§7.9 SR20");
    refused(&defined("DEFINE A AS TRUE").replace("(A B*)", "(A{0,0})"), "§7.9 SR20");
    refused("SELECT * FROM T MATCH_RECOGNIZE (ORDER BY D PATTERN (A) DEFINE A AS TRUE)", "§7.7 SR11");
    refused("SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY NOPE PATTERN (A) DEFINE A AS TRUE)", "§7.7 SR3");
    refused("SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D + 1 PATTERN (A) DEFINE A AS TRUE)", "§7.7 SR4");
    refused(
        "WITH RECURSIVE W (N) AS (SELECT 1 FROM T UNION ALL SELECT N FROM W MATCH_RECOGNIZE (MEASURES COUNT(*) AS C PATTERN (A) DEFINE A AS TRUE)) SELECT * FROM W",
        "§7.17 SR3f",
    );
}
