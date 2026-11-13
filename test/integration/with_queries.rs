//! The `<with clause>` of a query expression [ISO/IEC 9075-2:2023 §7.17,
//! Features T121, T122, T131, T132]: queries named for the query that
//! follows, in scope as its Syntax Rules say; WITH RECURSIVE, its strata
//! evaluated to their fixpoint, over every row or over the rows each
//! iteration adds, alike; its Syntax Rules, each refused; its iterations
//! bounded by EXECUTOR.MAX_ITERATIONS, its work tables spilled and its
//! statement cancelled; and in views, CHECK constraints, assertions,
//! INSERT and prepared statements.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::params::Params;
use turborel::value::{Decimal, Value};

/// The rows of `sql`, each its values joined by commas, or the SQLCODE
/// class of its refusal, as it opens or as a row is fetched.
fn outcome(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let mut cursor = s.open_cursor(sql).map_err(|e| e.code)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().map_err(|e| e.code)? {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// The rows of `sql`, which is not refused.
fn query(s: &Session, sql: &str) -> Vec<String> {
    outcome(s, sql).unwrap_or_else(|code| panic!("{sql} refused: {code:?}"))
}

/// The SQLCODE class of `sql`'s refusal, if it is refused.
fn refused(s: &Session, sql: &str) -> Option<Code> {
    outcome(s, sql).err()
}

/// The SQLCODE class of `sql`'s refusal as it is bound, and the rule it
/// cites.
fn broken(s: &Session, sql: &str) -> Option<(Code, String)> {
    s.open_cursor(sql).err().map(|e| (e.code, e.rule.unwrap_or_default()))
}

/// The SQLCODE class of a statement's refusal.
fn failed<T>(result: Result<T, SqlError>) -> Option<Code> {
    result.err().map(|e| e.code)
}

/// How `sql`'s recursive query is iterated, as EXPLAIN shows it.
fn iterated(s: &Session, sql: &str) -> &'static str {
    let shown = s.explain(sql, &Params::default()).unwrap();
    ["(delta distinct)", "(delta)", "(whole)"].into_iter().find(|word| shown.contains(word)).unwrap_or("none")
}

/// Schema A: T (K, V) of rows (1, 10), (2, 20), (3, 30); U (K, W) of rows
/// (1, 5), (3, 7); and E (A, B), the arcs of a graph of the cycle 1, 2, 3
/// and the arc 3, 4.
fn tables() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, V INTEGER) CREATE TABLE U (K INTEGER, W INTEGER) CREATE TABLE E (A INTEGER, B INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    a.run("INSERT INTO U VALUES (1, 5), (3, 7)").unwrap();
    a.run("INSERT INTO E VALUES (1, 2), (2, 3), (3, 1), (3, 4)").unwrap();
    a.commit().unwrap();
    db
}

/// Counting from 1 to `n` of INTEGER, by UNION ALL.
fn counting(n: u32) -> String {
    format!("WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < {n}) SELECT I FROM N")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3d, SR3i, GR2a, b]
/// a query named by a `<with list element>` is in scope in the query that
/// follows and in each element after it, of the columns its `<with column
/// list>` names or else its query's; one reads outer columns where it is
/// written, as a subquery, and is read by subqueries of the query; a WITH
/// in a derived table, a subquery and INSERT reads as at the top.
#[test]
fn a_with_list_names_its_queries_for_the_query_after_it_7_17() {
    let db = tables();
    let a = db.session("A");
    assert_eq!(query(&a, "WITH X AS (SELECT K, V FROM T WHERE V > 10) SELECT * FROM X ORDER BY K"), ["2,20", "3,30"]);
    assert_eq!(
        query(
            &a,
            "WITH X (P, Q) AS (SELECT K, V FROM T), Y AS (SELECT P FROM X WHERE Q < 30) SELECT * FROM Y, X WHERE Y.P = X.P ORDER BY 1"
        ),
        ["1,1,10", "2,2,20"],
        "a later element reads an earlier, of its column list",
    );
    assert_eq!(
        query(&a, "SELECT K FROM U WHERE EXISTS (WITH X AS (SELECT * FROM T WHERE T.K = U.K) SELECT * FROM X) ORDER BY K"),
        ["1", "3"]
    );
    assert_eq!(
        query(&a, "WITH X AS (SELECT * FROM T WHERE T.K < 3) SELECT K FROM U WHERE EXISTS (SELECT * FROM X WHERE X.K = U.K)"),
        ["1"]
    );
    assert_eq!(
        query(
            &a,
            "SELECT K, (WITH X AS (SELECT V FROM T WHERE T.K = U.K) SELECT MAX(V) FROM X WHERE EXISTS (SELECT 1 FROM X)) FROM U ORDER BY K"
        ),
        ["1,10", "3,30"],
        "correlated, read twice",
    );
    assert_eq!(query(&a, "SELECT * FROM (WITH X AS (SELECT K FROM T WHERE K > 2) SELECT K FROM X) D"), ["3"]);
    assert_eq!(query(&a, "WITH X AS (SELECT K FROM T) SELECT T.K FROM T WHERE T.K = 2"), ["2"], "a table of a with name's name");
    a.run("INSERT INTO U WITH X AS (SELECT K, V FROM T WHERE K = 2) SELECT * FROM X").unwrap();
    assert_eq!(query(&a, "SELECT * FROM U ORDER BY K"), ["1,5", "2,20", "3,7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3c, SR3d, SR3i] a
/// query named twice in one list, one of two columns of one name and no
/// column list, and one whose column list names more or fewer columns than
/// its query has are refused (SQLCODE -103, 2A000); a with name is not a
/// table of a schema, so qualified it names none (-103), and a
/// non-recursive list's element reads no element after it (-103), nor
/// itself.
#[test]
fn a_with_list_breaking_its_syntax_rules_is_refused_7_17() {
    let db = tables();
    let a = db.session("A");
    assert_eq!(refused(&a, "WITH X AS (SELECT K FROM T), X AS (SELECT V FROM T) SELECT * FROM X"), Some(Code::SyntaxRule), "SR3c");
    assert_eq!(refused(&a, "WITH X AS (SELECT K, K FROM T) SELECT * FROM X"), Some(Code::SyntaxRule), "SR3d i");
    assert_eq!(refused(&a, "WITH X (P) AS (SELECT K, K FROM T) SELECT * FROM X"), Some(Code::SyntaxRule), "SR3d ii, fewer");
    assert_eq!(refused(&a, "WITH X (P, Q) AS (SELECT K FROM T) SELECT * FROM X"), Some(Code::SyntaxRule), "SR3d ii, more");
    assert_eq!(query(&a, "WITH X (P, Q) AS (SELECT K, K FROM T) SELECT P FROM X WHERE Q = 1"), ["1"], "SR3d i, of a column list");
    assert_eq!(refused(&a, "WITH X AS (SELECT K FROM T) SELECT * FROM A.X"), Some(Code::SyntaxRule), "qualified");
    assert_eq!(refused(&a, "WITH X AS (SELECT K FROM Y), Y AS (SELECT K FROM T) SELECT * FROM X"), Some(Code::SyntaxRule), "SR3i, later");
    assert_eq!(refused(&a, "WITH X (P) AS (SELECT P FROM X) SELECT * FROM X"), Some(Code::SyntaxRule), "SR3i, itself");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR2c] a recursive query
/// is evaluated to its fixpoint: counting, of UNION ALL and of UNION; the
/// transitive closure of a graph with a cycle, of UNION, which keeps no
/// row twice and so ends; two queries each reading the other (EV, OD),
/// one stratum; a non-recursive query reading a recursive one, and one
/// read by it; a recursive query read twice, ordered, in a subquery, in a
/// derived table and correlated with an outer row; a recursive reference
/// in a derived table (SR3j ix 5 B); and its columns of the anchor's types
/// (SR19 b ii 2), a VARCHAR growing as it iterates.
#[test]
fn a_recursive_query_reaches_its_fixpoint_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 100").unwrap();
    assert_eq!(query(&a, &counting(5)), ["1", "2", "3", "4", "5"]);
    assert_eq!(
        query(&a, "WITH RECURSIVE N (I) AS (SELECT K FROM T WHERE K = 1 UNION SELECT I + 1 FROM N WHERE I < 5) SELECT I FROM N ORDER BY I"),
        ["1", "2", "3", "4", "5"]
    );
    let closure =
        "WITH RECURSIVE R (A, B) AS (SELECT A, B FROM E UNION SELECT R.A, E.B FROM R, E WHERE R.B = E.A) SELECT * FROM R ORDER BY A, B";
    assert_eq!(query(&a, closure), ["1,1", "1,2", "1,3", "1,4", "2,1", "2,2", "2,3", "2,4", "3,1", "3,2", "3,3", "3,4"]);
    let mutual = "WITH RECURSIVE EV (N) AS (VALUES (CAST(0 AS INTEGER)) UNION ALL SELECT N + 1 FROM OD WHERE N < 6), OD (N) AS (SELECT N + 1 FROM EV WHERE N < 6)";
    assert_eq!(query(&a, &format!("{mutual} SELECT * FROM EV ORDER BY N")), ["0", "2", "4", "6"]);
    assert_eq!(query(&a, &format!("{mutual} SELECT * FROM OD ORDER BY N")), ["1", "3", "5"]);
    let ten = "WITH RECURSIVE B (X) AS (VALUES (CAST(10 AS INTEGER))), N (I) AS (SELECT X FROM B UNION ALL SELECT I + 10 FROM N WHERE I < 30), M (J) AS (SELECT I * 2 FROM N)";
    assert_eq!(query(&a, &format!("{ten} SELECT * FROM M ORDER BY J")), ["20", "40", "60"], "strata in order");
    let three = "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < 3)";
    assert_eq!(query(&a, &format!("{three} SELECT * FROM N ORDER BY I DESC")), ["3", "2", "1"]);
    assert_eq!(query(&a, &format!("{three} SELECT * FROM N X, N Y WHERE X.I = Y.I ORDER BY 1")), ["1,1", "2,2", "3,3"]);
    assert_eq!(query(&a, &format!("{three} SELECT * FROM N WHERE I IN (SELECT I FROM N WHERE I > 1) ORDER BY I")), ["2", "3"]);
    assert_eq!(query(&a, &format!("SELECT * FROM ({three} SELECT * FROM N) D ORDER BY I")), ["1", "2", "3"]);
    let correlated = "SELECT K, (WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < U.K) SELECT COUNT(*) FROM N) FROM U";
    assert_eq!(query(&a, &format!("{correlated} ORDER BY K")), ["1,1", "3,3"]);
    let derived =
        "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM (SELECT I FROM N) D WHERE I < 3) SELECT * FROM N";
    assert_eq!(query(&a, derived), ["1", "2", "3"]);
    let grown = "WITH RECURSIVE N (I, S) AS (VALUES (1, CAST('a' AS VARCHAR(10))) UNION ALL SELECT I + 1, S || 'a' FROM N WHERE I < 3) SELECT * FROM N";
    assert_eq!(query(&a, grown), ["1,a", "2,aa", "3,aaa"]);
    assert_eq!(
        refused(&a, "WITH RECURSIVE N (I) AS (VALUES (1) UNION ALL SELECT I + 1 FROM N WHERE I < 20) SELECT * FROM N"),
        Some(Code::Data),
        "NUMERIC(1,0)"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR2c ii] [DESIGN] a
/// stratum of one member, a UNION of an anchor and a recursive operand
/// that distributes over its work table's rows, is iterated over the rows
/// each iteration adds (delta), of UNION keeping no row twice (delta
/// distinct); any other is iterated whole, as the General Rules say, and
/// both give the same rows. MC/DC of a node distributing: DISTINCT and
/// grouping distribute over UNION, not UNION ALL; a step of SELECT DISTINCT
/// over UNION ALL, iterated whole, ends where one iterated over the rows
/// added would not; two members are iterated whole.
#[test]
fn delta_iteration_gives_the_rows_whole_iteration_does_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 100").unwrap();
    assert_eq!(iterated(&a, &counting(5)), "(delta)");
    let distinct =
        "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION SELECT DISTINCT I + 1 FROM N WHERE I < 5) SELECT I FROM N ORDER BY I";
    let all = "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT DISTINCT I + 1 FROM N WHERE I < 5) SELECT I FROM N ORDER BY I";
    assert_eq!((iterated(&a, distinct), iterated(&a, all)), ("(delta distinct)", "(whole)"));
    assert_eq!(query(&a, distinct), query(&a, all));
    let grouped =
        "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION SELECT I + 1 FROM N WHERE I < 5 GROUP BY I) SELECT I FROM N ORDER BY I";
    assert_eq!(iterated(&a, grouped), "(delta distinct)");
    assert_eq!(query(&a, grouped), query(&a, distinct));
    let once = "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT DISTINCT I FROM N) SELECT I FROM N";
    assert_eq!(iterated(&a, once), "(whole)");
    assert_eq!(query(&a, once), ["1", "1"], "the second iteration adds no row");
    let closure = |union: &str, step: &str| {
        format!(
            "WITH RECURSIVE R (A, B) AS (SELECT A, B FROM E UNION {union} SELECT {step} R.A, E.B FROM R, E WHERE R.B = E.A AND R.A <> E.B) SELECT * FROM R ORDER BY A, B"
        )
    };
    assert_eq!((iterated(&a, &closure("ALL", "")), iterated(&a, &closure("", "DISTINCT"))), ("(delta)", "(delta distinct)"));
    assert_eq!(query(&a, &closure("ALL", "")), ["1,2", "1,3", "1,4", "2,1", "2,3", "2,4", "3,1", "3,2", "3,4"]);
    assert_eq!(query(&a, &closure("", "DISTINCT")), query(&a, &closure("ALL", "")));
    let mutual = "WITH RECURSIVE EV (N) AS (VALUES (CAST(0 AS INTEGER)) UNION ALL SELECT N + 1 FROM OD WHERE N < 6), OD (N) AS (SELECT N + 1 FROM EV WHERE N < 6) SELECT N FROM EV";
    assert_eq!(iterated(&a, mutual), "(whole)");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3h, SR3j i, vii,
/// viii, ix 2, x] a WITH RECURSIVE list breaking its Syntax Rules is
/// refused (SQLCODE -103, 2A000), each by the rule it breaks: an element
/// of no column list, even one not recursive; one inside another WITH
/// RECURSIVE list in an element; a stratum with no anchor, of one query,
/// of no UNION, of INTERSECT and of two queries; a stratum with an anchor
/// and a cycle of two queries that it is not on; and a query referencing
/// a query of its stratum twice in one `<query specification>`, directly
/// and through a derived table. MC/DC of the anchor rule (viii): a cycle
/// through an anchor is valid; of linear recursion: one reference is.
#[test]
fn a_recursive_list_needs_anchors_and_linear_recursion_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 20").unwrap();
    let cases = [
        ("SR3j i", "WITH RECURSIVE N AS (VALUES (1) UNION ALL SELECT 2 FROM T) SELECT * FROM N"),
        (
            "SR3h",
            "WITH RECURSIVE N (I) AS (VALUES (1) UNION ALL SELECT I + 1 FROM N WHERE I < 3), O (I) AS (WITH RECURSIVE P (J) AS (VALUES (1) UNION ALL SELECT J FROM P) SELECT J FROM P) SELECT * FROM N",
        ),
        ("SR3j vii", "WITH RECURSIVE N (I) AS (SELECT I FROM N) SELECT * FROM N"),
        ("SR3j vii", "WITH RECURSIVE N (I) AS (SELECT K FROM T, N) SELECT * FROM N"),
        ("SR3j vii", "WITH RECURSIVE N (I) AS (VALUES (1) INTERSECT SELECT I FROM N) SELECT * FROM N"),
        (
            "SR3j vii",
            "WITH RECURSIVE A1 (I) AS (VALUES (1) UNION ALL SELECT I FROM B1), B1 (I) AS (SELECT I FROM C1), C1 (I) AS (SELECT I FROM B1) SELECT * FROM A1",
        ),
        (
            "SR3j viii",
            "WITH RECURSIVE A1 (I) AS (VALUES (1) UNION ALL SELECT 1 FROM B1), B1 (I) AS (SELECT K FROM T, C1), C1 (I) AS (SELECT K FROM T, B1 UNION ALL SELECT I FROM A1) SELECT * FROM A1",
        ),
        ("SR3j ix 2, x", "WITH RECURSIVE N (I) AS (VALUES (1) UNION ALL SELECT X.I FROM N X, N Y) SELECT * FROM N"),
        ("SR3j ix 2, x", "WITH RECURSIVE N (I) AS (VALUES (1) UNION ALL SELECT X.I FROM N X, (SELECT I FROM N) Y) SELECT * FROM N"),
    ];
    for (clause, sql) in cases {
        assert_eq!(broken(&a, sql), Some((Code::SyntaxRule, format!("ISO/IEC 9075-2:2023 §7.17 {clause}"))), "{sql}");
    }
    let anchored = "WITH RECURSIVE A1 (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM B1 WHERE I < 4), B1 (I) AS (SELECT I FROM A1) SELECT * FROM A1 ORDER BY I";
    assert_eq!(query(&a, anchored), ["1", "2", "3", "4"], "SR3j viii, through an anchor");
    assert_eq!(
        query(&a, "WITH RECURSIVE N (I) AS (VALUES (1) UNION ALL SELECT I FROM T, N WHERE I = 9) SELECT * FROM N"),
        ["1"],
        "one reference"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3j ix 5 B to G] a
/// recursive reference is refused (SQLCODE -103, 2A000) outside a `<from
/// clause>` of the body or of a derived table there (B), in a subquery and
/// two derived tables deep; in a `<query specification>` with a set
/// function in its select list or HAVING (C), of its own or of a subquery
/// there; under INTERSECT ALL or EXCEPT ALL (D); as EXCEPT's second operand
/// (E), not its first; and on a side of an outer join given nulls, LEFT and RIGHT (F, G).
/// MC/DC of F: the preserved side of a LEFT join is valid.
#[test]
fn a_recursive_reference_is_restricted_to_its_body_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 5").unwrap();
    let step = |rest: &str| format!("WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL {rest}) SELECT * FROM N ORDER BY I");
    let code = |rest: &str| refused(&a, &step(rest));
    assert_eq!(code("SELECT I + 1 FROM N WHERE EXISTS (SELECT 1 FROM N)"), Some(Code::SyntaxRule), "B, a subquery");
    assert_eq!(code("SELECT I + 1 FROM (SELECT * FROM (SELECT I FROM N) F) D WHERE I < 3"), Some(Code::SyntaxRule), "B, deep");
    assert_eq!(code("SELECT COUNT(*) FROM N"), Some(Code::SyntaxRule), "C");
    assert_eq!(code("SELECT I + 1 FROM N WHERE I < 3 GROUP BY I HAVING MAX(I) > 0"), Some(Code::SyntaxRule), "C, HAVING");
    assert_eq!(code("SELECT (SELECT COUNT(*) FROM T) FROM N WHERE I < 3"), Some(Code::SyntaxRule), "C, a subquery");
    assert_eq!(code("SELECT I FROM N INTERSECT ALL SELECT I FROM N"), Some(Code::SyntaxRule), "D");
    assert_eq!(code("(SELECT I FROM N UNION ALL VALUES (2) EXCEPT ALL VALUES (3))"), Some(Code::SyntaxRule), "D, nested");
    assert_eq!(code("(SELECT K FROM T EXCEPT SELECT I FROM N)"), Some(Code::SyntaxRule), "E");
    assert_eq!(
        query(&a, &step("(SELECT I + 1 FROM N WHERE I < 3 EXCEPT SELECT K FROM T WHERE K = 9)")),
        ["1", "2", "3"],
        "E, the first operand"
    );
    assert_eq!(code("SELECT T.K FROM T LEFT JOIN N ON T.K = N.I"), Some(Code::SyntaxRule), "F");
    assert_eq!(code("SELECT I + 1 FROM N NATURAL RIGHT JOIN T WHERE I < 3"), Some(Code::SyntaxRule), "G");
    assert_eq!(query(&a, &step("SELECT I + 1 FROM N LEFT JOIN T ON N.I = T.K WHERE I < 3")), ["1", "2", "3"], "F, preserved");
    assert_eq!(
        query(&a, &step("SELECT I + 1 FROM N WHERE I < 3 AND I IN (SELECT K FROM T)")),
        ["1", "2", "3"],
        "B, a subquery of no reference"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3j vii, SR15]
/// [DESIGN] an anchor expression of UNION CORRESPONDING BY reads its
/// columns as the list names them; one of CORRESPONDING without BY, and
/// one whose UNION is within its body rather than its body, are not
/// supported (SQLCODE -900, 0A000), as CONFORMANCE.md says; an anchor
/// written as a WITH of its own is.
#[test]
fn some_anchor_expressions_are_not_supported_7_17() {
    let db = tables();
    let a = db.session("A");
    let by = "WITH RECURSIVE N (I, J) AS (SELECT K AS I, V AS J FROM T WHERE K = 1 UNION ALL CORRESPONDING BY (J, I) SELECT J + 1 AS J, I AS I FROM N WHERE I < 3) SELECT * FROM N";
    assert_eq!(query(&a, by), ["10,1"]);
    let without =
        "WITH RECURSIVE N (I) AS (SELECT K FROM T WHERE K = 1 UNION ALL CORRESPONDING SELECT I FROM N WHERE I < 3) SELECT * FROM N";
    assert_eq!(refused(&a, without), Some(Code::Unsupported));
    let within = "WITH RECURSIVE N (I) AS (VALUES (1) UNION ALL SELECT I + 1 FROM N WHERE I < 3 UNION ALL SELECT I + 10 FROM N WHERE I < 2) SELECT * FROM N";
    assert_eq!(refused(&a, within), Some(Code::Unsupported));
    let nested = "WITH RECURSIVE N (I) AS (WITH B (X) AS (VALUES (CAST(1 AS INTEGER))) SELECT X FROM B UNION ALL SELECT I + 1 FROM N WHERE I < 3) SELECT * FROM N";
    assert_eq!(query(&a, nested), ["1", "2", "3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR2c ii 4, NOTE 322]
/// [DESIGN] a recursive query taking more iterations than the session's
/// EXECUTOR.MAX_ITERATIONS is refused (SQLCODE -918), iterated over the
/// rows added and iterated whole alike, and its statement's work undone;
/// MC/DC of the limit: 10 rows take 10 iterations, allowed by a limit of
/// 10, 11 rows refused by it; 0 for none lets one of any number run.
#[test]
fn a_recursive_query_takes_at_most_its_iterations_7_17() {
    let db = tables();
    let a = db.session("A");
    let whole = |n: u32| {
        format!(
            "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT DISTINCT I + 1 FROM N WHERE I < {n}) SELECT COUNT(*) FROM N"
        )
    };
    a.run("SET EXECUTOR.MAX_ITERATIONS = 10").unwrap();
    assert_eq!(query(&a, &counting(10)).len(), 10);
    assert_eq!(refused(&a, &counting(11)), Some(Code::Resources));
    assert_eq!(query(&a, &whole(10)), ["10"]);
    assert_eq!(refused(&a, &whole(11)), Some(Code::Resources));
    assert_eq!(
        failed(a.run(&format!("INSERT INTO U {}", counting(11).replace("SELECT I FROM N", "SELECT I, I FROM N")))),
        Some(Code::Resources)
    );
    assert_eq!(query(&a, "SELECT COUNT(*) FROM U"), ["2"], "undone");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 0").unwrap();
    assert_eq!(query(&a, &counting(50)).len(), 50);
    assert_eq!(query(&a, &whole(30)), ["30"]);
}

/// Verifies: [DESIGN] RULES.md D44 a recursive query of no fixpoint and no
/// limit of its iterations is cancelled by STATEMENT.TIMEOUT_MS (SQLCODE
/// -920), and the session's next statement runs; a recursive query's work
/// tables spill past MEMORY.WORK_BYTES and give the same rows.
#[test]
fn a_recursive_query_is_cancelled_and_spills_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 0").unwrap();
    a.run("SET STATEMENT.TIMEOUT_MS = 50").unwrap();
    let endless = "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I FROM N) SELECT COUNT(*) FROM N";
    assert_eq!(refused(&a, endless), Some(Code::Cancelled));
    a.run("SET STATEMENT.TIMEOUT_MS = 0").unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T"), ["3"]);
    let sum = |union: &str| {
        format!(
            "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION {union} SELECT I + 1 FROM N WHERE I < 20000) SELECT COUNT(*), SUM(I) FROM N"
        )
    };
    let whole = "WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT DISTINCT I + 1 FROM N WHERE I < 300) SELECT COUNT(*), SUM(I) FROM N";
    let held = [query(&a, &sum("ALL")), query(&a, &sum("")), query(&a, whole)];
    a.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!([query(&a, &sum("ALL")), query(&a, &sum("")), query(&a, whole)], held);
    assert_eq!(held[0], ["20000,200010000"]);
    assert_eq!(held[2], ["300,45150"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17, §11.32 SR6] a view
/// may be defined by a WITH RECURSIVE query, and is read as its query; an
/// INSERT inserts the rows of one; a prepared one is executed again of
/// other parameters, each its own fixpoint.
#[test]
fn views_inserts_and_prepared_statements_read_recursive_queries_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("CREATE VIEW W (I) AS WITH RECURSIVE N (I) AS (SELECT K FROM T WHERE K = 1 UNION ALL SELECT I + 1 FROM N WHERE I < 3) SELECT I FROM N").unwrap();
    assert_eq!(query(&a, "SELECT * FROM W ORDER BY I"), ["1", "2", "3"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM W, T WHERE W.I = T.K"), ["3"]);
    a.run("INSERT INTO U WITH RECURSIVE N (I) AS (VALUES (CAST(10 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < 12) SELECT I, I FROM N").unwrap();
    assert_eq!(query(&a, "SELECT * FROM U ORDER BY K"), ["1,5", "3,7", "10,10", "11,11", "12,12"]);
    let p = a.prepare("WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < ?) SELECT COUNT(*), SUM(I) FROM N").unwrap();
    let run = |n: i128| {
        let mut cursor = a.open_prepared(&p, &[Value::Exact(Decimal::new(n, 0))]).unwrap();
        cursor.fetch().unwrap().map(|row| row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","))
    };
    assert_eq!(run(4), Some("4,10".to_owned()));
    assert_eq!(run(6), Some("6,21".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.6 GR3, ISO/IEC 9075-2:2023
/// §7.17] a CHECK constraint whose subquery reads no table, of a recursive
/// WITH or a non-recursive one, is checked of each row written (SQLCODE
/// -201 of a row it is false for), as one reading a table is; regression:
/// such a CHECK was refused as a subquery where none can be run (SQLCODE
/// -910). An assertion reading a recursive query refuses a row it is
/// false for.
#[test]
fn checks_and_assertions_read_recursive_queries_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("CREATE TABLE C (X INTEGER CHECK (X IN (WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < 3) SELECT I FROM N)))").unwrap();
    a.run("INSERT INTO C VALUES (2)").unwrap();
    assert_eq!(failed(a.run("INSERT INTO C VALUES (7)")), Some(Code::Constraint));
    assert_eq!(failed(a.run("UPDATE C SET X = 4")), Some(Code::Constraint));
    a.run("CREATE TABLE D (X INTEGER CHECK (X IN (WITH Y (I) AS (VALUES (1), (5)) SELECT I FROM Y)))").unwrap();
    a.run("INSERT INTO D VALUES (5)").unwrap();
    assert_eq!(failed(a.run("INSERT INTO D VALUES (2)")), Some(Code::Constraint));
    a.run("CREATE TABLE F (X INTEGER CHECK (X IN (SELECT K FROM T)))").unwrap();
    a.run("INSERT INTO F VALUES (2)").unwrap();
    a.commit().unwrap();
    let reached = "WITH RECURSIVE R (B) AS (VALUES (CAST(1 AS INTEGER)) UNION SELECT E.B FROM R, E WHERE R.B = E.A) SELECT B FROM R";
    a.run(&format!("CREATE ASSERTION REACHED CHECK (NOT EXISTS (SELECT * FROM U WHERE K NOT IN ({reached})))")).unwrap();
    a.run("INSERT INTO U VALUES (4, 0)").unwrap();
    assert_eq!(failed(a.run("INSERT INTO U VALUES (5, 0)")), Some(Code::Constraint));
    a.run("INSERT INTO E VALUES (4, 5)").unwrap();
    a.run("INSERT INTO U VALUES (5, 0)").unwrap();
    assert_eq!(failed(a.run("DELETE FROM E WHERE A = 4")), Some(Code::Constraint), "a row of U no longer reached");
}
