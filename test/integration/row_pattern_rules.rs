//! The Syntax Rules of row pattern recognition, each condition's halves
//! [ISO/IEC 9075-2:2023 §6.7 SR9, §6.9 SR11, SR12, §6.27 SR4 to SR6,
//! §7.7 SR3, §10.9 SR15, §4.15]: what each accepts beside what it
//! refuses, so a rule decided by one condition is seen deciding it.

use turborel::db::Session;
use turborel::error::Code;

use crate::common::{expected, prices, recognized, rows};

/// The V of a start, prices falling, then rising.
const V: &str = "STRT DOWN+ UP+";

/// Asserts that `sql` binds and runs.
fn accepted(a: &Session, sql: &str, why: &str) {
    assert!(rows(a, sql).is_ok(), "{why}: {sql}");
}

/// Asserts that `sql` is refused, SQLCODE -103.
fn refused(a: &Session, sql: &str, why: &str) {
    assert_eq!(rows(a, sql).err(), Some(Code::SyntaxRule), "{why}: {sql}");
}

/// ONE ROW PER MATCH of V, of the measures `measures`.
fn one(measures: &str) -> String {
    recognized(measures, "ONE ROW PER MATCH", "SKIP PAST LAST ROW", V)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 SR4, §10.9 SR15] a
/// navigation's value expression, or a set function's argument, of several
/// column references each of one variable is accepted, of two variables
/// refused. MC/DC of `found.is_some_and(|v| v != variable)`: a variable
/// found and another refuses; one found and the same accepts; none found
/// yet accepts.
#[test]
fn one_variable_of_a_navigation_or_an_argument_6_27() {
    let (_db, a) = prices();
    let q = format!("{} WHERE TICKER = 'A' ORDER BY P", one("PREV(DOWN.PRICE + DOWN.D) AS P, SUM(DOWN.PRICE + DOWN.D) AS S"));
    assert_eq!(rows(&a, &q), expected(&["A,11,22", "A,17,17"]));
    accepted(&a, &one("PREV(DOWN.PRICE) AS P"), "§6.27 SR4, none found yet");
    refused(&a, &one("PREV(DOWN.PRICE + UP.D) AS P"), "§6.27 SR4");
    refused(&a, &one("SUM(DOWN.PRICE + UP.D) AS S"), "§10.9 SR15");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.7 SR9, §4.26] an ordered
/// set function's argument not aggregated, in a measure, reads columns
/// functionally dependent on the partitioning columns. MC/DC of
/// `partition.contains(&column) || known.determines(..)`: a partitioning
/// column accepts; a column the primary key of the partitioning columns
/// determines accepts; a column neither refuses. MC/DC of `(Measure,
/// ordered)`: the same column aggregated by an unordered set function is
/// accepted. An ordered set function is a measure's, of the rows of its
/// match, as an unordered one is [§10.9 GR1a to c].
#[test]
fn an_ordered_set_functions_argument_is_of_the_partition_6_7() {
    let (_db, a) = prices();
    let over = |partition: &str, measure: &str| {
        format!(
            "SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY {partition} ORDER BY D MEASURES {measure} AS X PATTERN (P+) DEFINE P AS TRUE) AS R"
        )
    };
    let fraction = |column: &str| format!("PERCENTILE_CONT(CASE WHEN {column} > 9 THEN 0.5 ELSE 0.0 END) WITHIN GROUP (ORDER BY PRICE)");
    let q = format!("{} ORDER BY TICKER", over("TICKER", &fraction("CHARACTER_LENGTH(TICKER) * 10")));
    assert_eq!(rows(&a, &q), expected(&["A,1E1", "B,5E0"]), "a partitioning column");
    accepted(&a, &over("TICKER, D", &fraction("PRICE")), "PRICE determined by the primary key (TICKER, D)");
    refused(&a, &over("TICKER", &fraction("PRICE")), "§6.7 SR9, PRICE of no partitioning column's key");
    refused(&a, &over("TICKER", &fraction("D")), "§6.7 SR9, D of no partitioning column's key");
    refused(&a, &over("TICKER", "RANK(D) WITHIN GROUP (ORDER BY PRICE)"), "§6.7 SR9, a hypothetical set function");
    let ranked = format!("{} ORDER BY TICKER", over("TICKER", "RANK(9) WITHIN GROUP (ORDER BY PRICE)"));
    assert_eq!(rows(&a, &ranked), expected(&["A,2", "B,4"]), "a hypothetical set function of a constant");
    accepted(&a, &over("TICKER", "SUM(D)"), "an unordered set function aggregates its argument");
    let windowed = "SELECT D, X OVER W FROM T WHERE TICKER = 'A' WINDOW W AS (PARTITION BY TICKER ORDER BY D MEASURES \
                    PERCENTILE_CONT(CASE WHEN TICKER = 'A' THEN 1.0 ELSE 0.0 END) WITHIN GROUP (ORDER BY PRICE) AS X \
                    ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING PATTERN (P+) DEFINE P AS TRUE) ORDER BY D";
    assert_eq!(rows(&a, windowed).map(|r| r.first().cloned()), Ok(Some("1,1.2E1".to_owned())), "a window's partitioning column");
    refused(&a, &windowed.replace("TICKER = 'A' THEN", "D = 1 THEN"), "§6.7 SR9 of a window");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 SR5b, §6.9 SR12] FINAL
/// is refused in a definition alone; RUNNING is accepted there and FINAL in
/// a measure. MC/DC of `final_ && define` and of `semantics == Final &&
/// define`: FINAL in a definition refuses; RUNNING in a definition accepts;
/// FINAL in a measure accepts.
#[test]
fn final_in_a_definition_alone_is_refused_6_27() {
    let (_db, a) = prices();
    let defined = |define: &str, measure: &str| {
        format!("SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES {measure} AS N PATTERN (P Q*) DEFINE {define})")
    };
    refused(&a, &defined("Q AS FINAL LAST(PRICE) > 0", "COUNT(*)"), "§6.27 SR5b");
    accepted(&a, &defined("Q AS RUNNING LAST(PRICE) > 0", "COUNT(*)"), "§6.27 SR5b, RUNNING");
    accepted(&a, &defined("Q AS TRUE", "FINAL LAST(PRICE)"), "§6.27 SR5b, FINAL in a measure");
    refused(&a, &defined("Q AS FINAL COUNT(*) > 0", "COUNT(*)"), "§6.9 SR12");
    let runs = "SELECT S, E FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES FIRST(D) AS S, LAST(D) AS E \
                PATTERN (X+) DEFINE X AS RUNNING COUNT(*) <= 2) WHERE TICKER = 'A' ORDER BY S";
    assert_eq!(rows(&a, runs), expected(&["1,2", "3,4", "5,6", "7,8", "9,9"]), "§6.9 SR12, RUNNING");
    accepted(&a, &defined("Q AS TRUE", "FINAL COUNT(*)"), "§6.9 SR12, FINAL in a measure");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 SR5c i, SR6a,
/// X3.135-1986 §5.2, §5.3 SR1] a literal offset is not negative: -1 is
/// refused, -0 and +1 accepted, a sign written against its number being of
/// the literal; a sign apart from its number is no literal, SQLSTATE 42601.
/// MC/DC of `units < 0` of an exact literal: -1 refuses; -0 accepts.
#[test]
fn a_literal_offset_is_not_negative_6_27() {
    let (_db, a) = prices();
    refused(&a, &one("PREV(PRICE, -1) AS P"), "SR6a");
    assert_eq!(rows(&a, &one("PREV(PRICE, - 1) AS P")).err(), Some(Code::Syntax), "the sign apart");
    let q = |offset: &str| format!("{} WHERE TICKER = 'B'", one(&format!("PREV(PRICE, {offset}) AS P")));
    assert_eq!(rows(&a, &q("-0")), expected(&["B,5"]));
    assert_eq!(rows(&a, &q("+1")), expected(&["B,4"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.9 SR11b] RUNNING or FINAL
/// is written in row pattern matching alone, in a measure's subquery too.
/// MC/DC of `semantics.is_some() && !in_pattern()`: RUNNING outside
/// refuses; RUNNING in a measure's subquery accepts; no semantics outside
/// accepts.
#[test]
fn running_or_final_in_row_pattern_matching_alone_6_9() {
    let (_db, a) = prices();
    refused(&a, "SELECT RUNNING COUNT(*) FROM T", "§6.9 SR11b");
    refused(&a, "SELECT (SELECT FINAL COUNT(*) FROM T) FROM T", "§6.9 SR11b, in a subquery");
    let q = format!("{} WHERE TICKER = 'B'", one("(SELECT RUNNING COUNT(*) FROM T) AS N"));
    assert_eq!(rows(&a, &q), expected(&["B,12"]));
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T"), expected(&["12"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 SR3a ii 2 A] a row
/// pattern partitioning column has a COLLATE only of a character string
/// type. MC/DC of `collate.is_some() && !is_character()`: COLLATE of
/// INTEGER refuses; COLLATE of CHARACTER accepts; INTEGER of no COLLATE
/// accepts.
#[test]
fn a_partitioning_columns_collate_7_7() {
    let (_db, a) = prices();
    let over = |partition: &str| {
        format!("SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY {partition} MEASURES COUNT(*) AS N PATTERN (P+) DEFINE P AS TRUE) AS R")
    };
    let q = format!("{} ORDER BY 1", over("TICKER COLLATE CASE_INSENSITIVE"));
    assert_eq!(rows(&a, &q), expected(&["A,9", "B,3"]));
    refused(&a, &over("D COLLATE CASE_INSENSITIVE"), "§7.7 SR3a ii 2 A");
    accepted(&a, &over("D"), "no COLLATE");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.15, §7.6 SR8b ii] a view
/// of ALL ROWS PER MATCH: a partitioning column not null in its table is
/// not nullable; a measure and another input column are.
#[test]
fn nullability_of_a_view_of_all_rows_7_6() {
    let (_db, a) = prices();
    let view = recognized("COUNT(*) AS N", "ALL ROWS PER MATCH", "SKIP PAST LAST ROW", V);
    crate::common::exec(&a, &format!("CREATE VIEW VR AS {view}")).unwrap();
    let nullable = "SELECT COLUMN_NAME, IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'VR' ORDER BY ORDINAL_POSITION";
    assert_eq!(rows(&a, nullable), expected(&["TICKER,NO", "D,NO", "N,YES", "PRICE,YES"]));
}
