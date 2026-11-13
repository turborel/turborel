//! LISTAGG and ANY_VALUE [ISO/IEC 9075-2:2023 §10.9, Features T625 and
//! T626]: a group's strings joined in the order WITHIN GROUP gives, by
//! keys of either direction and collation, nulls removed, DISTINCT by the
//! value's collation, each overflow behavior; a value of each group; both
//! of no rows, in grouping sets, views and prepared statements, past the
//! work area; each Syntax Rule; and neither in a constraint.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError, SqlState};
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, both_ways, exec, failure, fetched, rows, sorted};

/// Schema A: W, of names in groups, each with a key, a null name and a
/// group of none among them; E, empty, of the same columns.
fn names() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE W (G INTEGER, NAME VARCHAR(10), K INTEGER)
           CREATE TABLE E (G INTEGER, NAME VARCHAR(10), K INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO W VALUES (1, 'b', 2), (1, 'a', 1), (1, 'c', 3), (1, NULL, 4), (2, 'X', 1), (2, 'x', 2), (2, 'y', 3), (3, NULL, 1)")
        .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, in order.
fn ordered(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11a, GR11b, GR11e,
/// GR11f] LISTAGG joins each group's values by its separator in the order
/// of its sort keys, ascending, descending, several, a key not the value
/// among them, a key by its collation; a null value is removed, with SQLSTATE 01003; of one value
/// it is that value, and of none the null value, of a group or of an empty
/// table.
#[test]
fn values_are_joined_in_order_10_9() {
    let (_db, a) = names();
    let by_key = "SELECT G, LISTAGG(NAME, ', ') WITHIN GROUP (ORDER BY K) FROM W GROUP BY G ORDER BY G";
    assert_eq!(ordered(&a, by_key), ["1,a, b, c", "2,X, x, y", "3,NULL"]);
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null eliminated");
    let descending = "SELECT LISTAGG(NAME, '') WITHIN GROUP (ORDER BY NAME DESC) FROM W WHERE G = 1";
    assert_eq!(ordered(&a, descending), ["cba"]);
    let several = "SELECT LISTAGG(NAME, '-') WITHIN GROUP (ORDER BY G DESC, K DESC) FROM W";
    assert_eq!(ordered(&a, several), ["y-x-X-c-b-a"]);
    let one = "SELECT LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY K) FROM W WHERE K = 3 AND G = 1";
    assert_eq!(ordered(&a, one), ["c"]);
    assert_eq!(ordered(&a, "SELECT LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY K) FROM E"), ["NULL"]);
    let collated = "SELECT LISTAGG(NAME, '') WITHIN GROUP (ORDER BY NAME COLLATE CASE_INSENSITIVE, K DESC) FROM W WHERE G = 2";
    assert_eq!(ordered(&a, collated), ["xXy"], "X and x alike by the key's collation, then by K descending");
    let all = "SELECT LISTAGG(ALL NAME, ',') WITHIN GROUP (ORDER BY K, G) FROM W WHERE G = 2";
    assert_eq!(ordered(&a, all), ["X,x,y"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11c, §8.2] LISTAGG
/// DISTINCT removes values equal by the value's collation, keeping the
/// copy first in its order: so 'X' and 'x' are two values, but one under
/// CASE_INSENSITIVE, the one first by K, either direction.
#[test]
fn distinct_keeps_the_first_of_values_alike_10_9() {
    let (_db, a) = names();
    let cased = "SELECT LISTAGG(DISTINCT NAME, ',') WITHIN GROUP (ORDER BY K) FROM W WHERE G = 2";
    assert_eq!(ordered(&a, cased), ["X,x,y"]);
    let ascending = "SELECT LISTAGG(DISTINCT NAME COLLATE CASE_INSENSITIVE, ',') WITHIN GROUP (ORDER BY K) FROM W WHERE G = 2";
    assert_eq!(ordered(&a, ascending), ["X,y"]);
    let descending = "SELECT LISTAGG(DISTINCT NAME COLLATE CASE_INSENSITIVE, ',') WITHIN GROUP (ORDER BY K DESC) FROM W WHERE G = 2";
    assert_eq!(ordered(&a, descending), ["y,x"]);
    a.run("INSERT INTO W VALUES (4, 'p', 1), (4, 'p', 2), (4, 'q', 3)").unwrap();
    let repeated = "SELECT LISTAGG(DISTINCT NAME, ',') WITHIN GROUP (ORDER BY NAME) FROM W WHERE G = 4";
    assert_eq!(ordered(&a, repeated), ["p,q"]);
    a.rollback().unwrap();
}

/// Schema A: L, of three values of 16,000 characters, each ending in its
/// letter, and two of 32,767.
fn long_values() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE L (N INTEGER, S VARCHAR(32767))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO L VALUES (1, LPAD('x', 16000, 'a')), (1, LPAD('y', 16000, 'a')), (1, LPAD('z', 16000, 'a'))").unwrap();
    a.run("INSERT INTO L VALUES (2, LPAD('x', 32767, 'a')), (2, LPAD('y', 32767, 'a'))").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The length and last 8 characters of LISTAGG(S, ',' `overflow`) of the
/// rows of L whose N is `n`, in the order of S.
fn joined(a: &Session, n: i32, overflow: &str) -> Result<Vec<String>, Code> {
    let listagg = format!("LISTAGG(S, ','{overflow}) WITHIN GROUP (ORDER BY S)");
    let sql = format!("SELECT CHAR_LENGTH(J), SUBSTRING(J FROM CHAR_LENGTH(J) - 7) FROM (SELECT {listagg} FROM L WHERE N = {n}) AS T (J)");
    rows(a, &sql)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR11f, SR11g, GR11g]
/// values joined longer than 32,767 characters are SQLSTATE 22001 of ON
/// OVERFLOW ERROR, written or implicit; ON OVERFLOW TRUNCATE takes the
/// most values that fit with the separator and filler, `...` if none is
/// written, and WITH COUNT the count of those left out; of none that fit,
/// the separator and filler alone, the count after them.
#[test]
fn overflow_errs_or_truncates_10_9() {
    let (_db, a) = long_values();
    assert_eq!(joined(&a, 1, ""), Err(Code::Data));
    let written = fetched(&a, "SELECT LISTAGG(S, ',' ON OVERFLOW ERROR) WITHIN GROUP (ORDER BY S) FROM L WHERE N = 1");
    assert_eq!(failure(written), (Code::Data, "22001".to_owned()));
    assert_eq!(joined(&a, 1, " ON OVERFLOW TRUNCATE WITHOUT COUNT").unwrap(), ["32005,aaay,..."]);
    assert_eq!(joined(&a, 1, " ON OVERFLOW TRUNCATE '~' WITH COUNT").unwrap(), ["32006,aay,~(1)"]);
    assert_eq!(joined(&a, 2, " ON OVERFLOW TRUNCATE WITHOUT COUNT").unwrap(), ["4,,..."]);
    assert_eq!(joined(&a, 2, " ON OVERFLOW TRUNCATE WITH COUNT").unwrap(), ["7,,...(2)"]);
    let fits =
        "SELECT CHAR_LENGTH(LISTAGG(S, ',' ON OVERFLOW TRUNCATE WITH COUNT) WITHIN GROUP (ORDER BY S)) FROM L WHERE N = 1 AND S LIKE '%x'";
    assert_eq!(ordered(&a, fits), ["16000"], "no overflow, no filler");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11g iii 2) A) I),
/// B) I)] a filler so long that the separator and it are longer than
/// 32,767 characters gives three periods, and WITH COUNT the count after
/// them.
#[test]
fn a_filler_too_long_gives_periods_10_9() {
    let (_db, a) = long_values();
    let filler = "f".repeat(32767);
    assert_eq!(joined(&a, 2, &format!(" ON OVERFLOW TRUNCATE '{filler}' WITHOUT COUNT")).unwrap(), ["3,..."]);
    assert_eq!(joined(&a, 2, &format!(" ON OVERFLOW TRUNCATE '{filler}' WITH COUNT")).unwrap(), ["6,...(2)"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR7d ii, v, SR7c,
/// SR7f] ANY_VALUE is a value of the group, of its argument's type: of a
/// column of one value in each group that value, row by row and by
/// batches; nulls removed with 01003; DISTINCT allowed; of no rows null;
/// of an array, an array.
#[test]
fn any_value_is_one_of_the_group_10_9() {
    let (_db, a) = names();
    a.run("CREATE TABLE D (G INTEGER, TITLE VARCHAR(10))").unwrap();
    a.run("INSERT INTO D VALUES (1, 'one'), (1, 'one'), (1, NULL), (2, 'two')").unwrap();
    let (batched, by_rows) = both_ways(&a, "SELECT G, ANY_VALUE(TITLE) FROM D GROUP BY G");
    assert_eq!(batched, ["1,one", "2,two"]);
    assert_eq!(by_rows, batched);
    assert_eq!(ordered(&a, "SELECT ANY_VALUE(TITLE) FROM D WHERE G = 1"), ["one"]);
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null eliminated");
    assert_eq!(ordered(&a, "SELECT ANY_VALUE(DISTINCT TITLE) FROM D WHERE G = 2"), ["two"]);
    assert_eq!(ordered(&a, "SELECT ANY_VALUE(K), COUNT(*) FROM E"), ["NULL,0"]);
    assert_eq!(ordered(&a, "SELECT CARDINALITY(ANY_VALUE(ARRAY[G, G])) FROM D WHERE G = 2"), ["2"]);
    let dependent = "SELECT D.G, ANY_VALUE(D.TITLE), COUNT(*) FROM D, W WHERE D.G = W.G AND D.TITLE IS NOT NULL GROUP BY D.G";
    assert_eq!(sorted(&a, dependent), ["1,one,8", "2,two,3"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9, §7.13, §11.32] LISTAGG
/// in grouping sets joins each set's groups, the total's of every row;
/// over a view and in a prepared statement it gives the same; EXPLAIN shows
/// the call with its separator and keys.
#[test]
fn listagg_in_grouping_views_and_statements_10_9() {
    let (_db, a) = names();
    let rolled = "SELECT G, LISTAGG(NAME, '') WITHIN GROUP (ORDER BY NAME) FROM W WHERE G < 3 GROUP BY ROLLUP (G)";
    assert_eq!(sorted(&a, rolled), ["1,abc", "2,Xxy", "NULL,Xabcxy"]);
    a.run("CREATE VIEW V (G, L) AS SELECT G, LISTAGG(NAME, '/') WITHIN GROUP (ORDER BY K DESC) FROM W GROUP BY G").unwrap();
    assert_eq!(sorted(&a, "SELECT L FROM V WHERE G = 1"), ["c/b/a"]);
    let p = a.prepare("SELECT LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY K) FROM W WHERE G = ?").unwrap();
    let mut cursor = a.open_prepared(&p, &[Value::Exact(Decimal::new(2, 0))]).unwrap();
    let row = cursor.fetch().unwrap().expect("a row");
    assert!(matches!(row.first(), Some(Value::Char(s)) if s == "X,x,y"), "{row:?}");
    drop(cursor);
    let plan = a.explain("SELECT LISTAGG(NAME, ';') WITHIN GROUP (ORDER BY K DESC) FROM W", &Params::new()).unwrap();
    assert!(plan.contains("LISTAGG(") && plan.contains("';'") && plan.contains("2 DESC"), "{plan}");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9] [DESIGN] RULES.md
/// D38: LISTAGG and ANY_VALUE of many groups give the same, past the
/// work area, as in memory.
#[test]
fn listagg_past_the_work_area_10_9() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (G INTEGER, NAME VARCHAR(10), K INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0..4000).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(|i| format!("({}, 'n{}', {})", i % 997, i % 13, i)).collect();
        a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let sql = "SELECT G, LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY K DESC), LISTAGG(DISTINCT NAME, ',') WITHIN GROUP (ORDER BY NAME), \
               ANY_VALUE(G * 2) FROM M GROUP BY G";
    let in_memory = sorted(&a, sql);
    assert_eq!(in_memory.len(), 997);
    assert_eq!(counted.taken(), 0, "the default MEMORY.WORK_BYTES holds them");
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!(sorted(&a, sql), in_memory);
    assert!(counted.taken() > 0, "they spilled");
}

/// The SQLCODE of `failed`, and whether its message or rule cites `rule`.
fn cites<T: std::fmt::Debug>(failed: Result<T, SqlError>, rule: &str) -> (Code, bool) {
    let e = failed.expect_err("a failure");
    let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
    (e.code, cited.contains(rule))
}

/// The SQLCODE of the query `sql`, and whether its error cites `rule`.
fn refused(a: &Session, sql: &str, rule: &str) -> (Code, bool) {
    cites(fetched(a, sql), rule)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 F, SR11a, SR11e,
/// SR11g, §9.14 SR3] LISTAGG of a value not a character string, with a set
/// function or a subquery in its value or a key, with a key of an array,
/// with a separator or filler longer than its result, is SQLCODE -103;
/// without WITHIN GROUP, or with a separator not a literal, -102.
#[test]
fn listagg_misused_is_refused_10_9() {
    let (_db, a) = names();
    let within = "WITHIN GROUP (ORDER BY K)";
    let cases = [
        (format!("SELECT LISTAGG(K, ',') {within} FROM W"), Code::SyntaxRule, "§10.9 F"),
        ("SELECT LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY (SELECT 1 FROM E)) FROM W".to_owned(), Code::SyntaxRule, "§10.9 SR11a"),
        ("SELECT LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY COUNT(*)) FROM W".to_owned(), Code::SyntaxRule, "§10.9 SR11a"),
        (format!("SELECT LISTAGG(NAME || (SELECT MAX(NAME) FROM E), ',') {within} FROM W"), Code::SyntaxRule, "§10.9 SR11a"),
        ("SELECT LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY ARRAY[K]) FROM W".to_owned(), Code::SyntaxRule, "§9.14 SR3"),
        (format!("SELECT LISTAGG(NAME, '{}') {within} FROM W", ",".repeat(32768)), Code::SyntaxRule, "§10.9 SR11e"),
        (
            format!("SELECT LISTAGG(NAME, ',' ON OVERFLOW TRUNCATE '{}' WITH COUNT) {within} FROM W", ".".repeat(32768)),
            Code::SyntaxRule,
            "§10.9 SR11g",
        ),
        ("SELECT LISTAGG(NAME, ',') FROM W".to_owned(), Code::Syntax, "WITHIN GROUP"),
        (format!("SELECT LISTAGG(NAME, NAME) {within} FROM W"), Code::Syntax, "<listagg separator>"),
        (format!("SELECT LISTAGG(NAME, ',' ON OVERFLOW TRUNCATE) {within} FROM W"), Code::Syntax, "WITH COUNT"),
    ];
    for (sql, code, rule) in &cases {
        assert_eq!(refused(&a, sql, rule), (*code, true), "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16 SR1ap, SR1ar, SR1as,
/// §11.9 CR2] ANY_VALUE, ARRAY_AGG and LISTAGG are potential sources of
/// non-determinism, so neither a CHECK nor an assertion may contain one:
/// SQLCODE -103; MAX of a number may.
#[test]
fn nondeterministic_set_functions_constrain_nothing_10_9() {
    let (_db, a) = names();
    let cases = [
        ("ANY_VALUE(K)", "§9.16 SR1ap"),
        ("CARDINALITY(ARRAY_AGG(K))", "§9.16 SR1ar"),
        ("CHAR_LENGTH(LISTAGG(NAME, ',') WITHIN GROUP (ORDER BY K))", "§9.16 SR1as"),
    ];
    for (call, rule) in cases {
        let assertion = format!("CREATE ASSERTION N CHECK ((SELECT {call} FROM W) < 100)");
        assert_eq!(refused_statement(&a, &assertion, rule), (Code::SyntaxRule, true), "{assertion}");
        let check = format!("CREATE TABLE C (X INTEGER CHECK (X < (SELECT {call} FROM W)))");
        assert_eq!(refused_statement(&a, &check, rule), (Code::SyntaxRule, true), "{check}");
    }
    a.run("CREATE ASSERTION N CHECK ((SELECT MAX(K) FROM W) < 100)").unwrap();
    a.rollback().unwrap();
}

/// The SQLCODE of the statement `sql`, and whether its error cites `rule`.
fn refused_statement(a: &Session, sql: &str, rule: &str) -> (Code, bool) {
    cites(a.run(sql), rule)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9, Annex B] ANY_VALUE,
/// LISTAGG, OVERFLOW and WITHIN, which SQL-92 does not reserve, name
/// columns; they are key words only where the functions' syntax has them.
#[test]
fn listagg_words_name_columns_10_9() {
    let (_db, a) = names();
    a.run("CREATE TABLE K (ANY_VALUE INTEGER, LISTAGG VARCHAR(5), OVERFLOW INTEGER, WITHIN INTEGER)").unwrap();
    a.run("INSERT INTO K VALUES (1, 'l', 2, 3)").unwrap();
    assert_eq!(ordered(&a, "SELECT ANY_VALUE, LISTAGG, OVERFLOW, WITHIN FROM K"), ["1,l,2,3"]);
    let called = "SELECT ANY_VALUE(ANY_VALUE), LISTAGG(LISTAGG, ',' ON OVERFLOW ERROR) WITHIN GROUP (ORDER BY WITHIN) FROM K";
    assert_eq!(ordered(&a, called), ["1,l"]);
    a.rollback().unwrap();
}
