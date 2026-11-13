//! Grouping sets [ISO/IEC 9075-2:2023 §7.13, §6.9, Features T431 to T434]:
//! ROLLUP, CUBE, GROUPING SETS nested and concatenated, the empty grouping
//! set, GROUPING of one column and of several, GROUP BY DISTINCT and ALL;
//! a column a set leaves out null in it, set functions' arguments too;
//! HAVING, DISTINCT, ORDER BY, collations, subqueries and views over them;
//! an empty table; each Syntax Rule; and the grouping past the work area.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, exec, fetched, rows, sorted};

/// Schema A: S, of sales in regions and years, a null region among them;
/// E, empty, of the same columns; ONE, of one row.
fn sales() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE S (R VARCHAR(5), Y INTEGER, AMOUNT INTEGER)
           CREATE TABLE E (R VARCHAR(5), Y INTEGER, AMOUNT INTEGER)
           CREATE TABLE ONE (X INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO S VALUES ('n', 1, 10), ('n', 2, 20), ('s', 1, 5), ('s', 1, 7), (NULL, 2, 1)").unwrap();
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, sorted.
fn of(s: &Session, sql: &str) -> Vec<String> {
    sorted(s, sql)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR13, SR17] ROLLUP (R,
/// Y) groups by (R, Y), by (R) and by (): each set's groups, a column it
/// leaves out null, GROUPING of it 1. A null region of the data is a
/// group of its own, GROUPING 0, told from the region the total leaves
/// out by GROUPING alone.
#[test]
fn rollup_gives_subtotals_and_the_total_7_13() {
    let (_db, a) = sales();
    let got = of(&a, "SELECT R, Y, SUM(AMOUNT), GROUPING(R), GROUPING(Y) FROM S GROUP BY ROLLUP (R, Y)");
    let want =
        ["NULL,2,1,0,0", "NULL,NULL,1,0,1", "NULL,NULL,43,1,1", "n,1,10,0,0", "n,2,20,0,0", "n,NULL,30,0,1", "s,1,12,0,0", "s,NULL,12,0,1"];
    assert_eq!(got, want);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR14, SR15, SR16, §6.9
/// SR10c] CUBE (R, Y) groups by every subset of (R, Y); GROUPING (R, Y) is
/// twice GROUPING (R) plus GROUPING (Y). Two GROUPING SETS concatenated
/// give the sets of each with each, the CUBE's here; a GROUPING SETS
/// nested in one is promoted; a column with a ROLLUP concatenates it
/// with each of the ROLLUP's sets.
#[test]
fn cubes_and_grouping_sets_combine_7_13() {
    let (_db, a) = sales();
    let cube = "SELECT R, Y, COUNT(*), GROUPING(R, Y) FROM S GROUP BY CUBE (R, Y)";
    let want = [
        "NULL,1,3,2",
        "NULL,2,1,0",
        "NULL,2,2,2",
        "NULL,NULL,1,1",
        "NULL,NULL,5,3",
        "n,1,1,0",
        "n,2,1,0",
        "n,NULL,2,1",
        "s,1,2,0",
        "s,NULL,2,1",
    ];
    assert_eq!(of(&a, cube), want);
    let concatenated = "SELECT R, Y, COUNT(*), GROUPING(R, Y) FROM S GROUP BY GROUPING SETS ((R), ()), GROUPING SETS ((Y), ())";
    assert_eq!(of(&a, concatenated), want);
    let nested = "SELECT R, Y, COUNT(*), GROUPING(R), GROUPING(Y) FROM S GROUP BY GROUPING SETS (R, GROUPING SETS (Y, ()))";
    let promoted = ["NULL,1,3,1,0", "NULL,2,2,1,0", "NULL,NULL,1,0,1", "NULL,NULL,5,1,1", "n,NULL,2,0,1", "s,NULL,2,0,1"];
    assert_eq!(of(&a, nested), promoted);
    let rolled = ["NULL,2,1", "NULL,NULL,1", "n,1,1", "n,2,1", "n,NULL,2", "s,1,2", "s,NULL,2"];
    assert_eq!(of(&a, "SELECT R, Y, COUNT(*) FROM S GROUP BY R, ROLLUP (Y)"), rolled);
    let pairs = "SELECT R, Y, COUNT(*) FROM S GROUP BY ROLLUP ((R, Y))";
    assert_eq!(of(&a, pairs), ["NULL,2,1", "NULL,NULL,5", "n,1,1", "n,2,1", "s,1,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR6, SR17n, Feature
/// T434] GROUP BY ALL, as with no quantifier, gives each of two sets alike
/// its groups, as UNION ALL keeps both their rows; GROUP BY DISTINCT gives
/// them once, as UNION DISTINCT does. EXPLAIN shows sets alike made one
/// before grouping, and rows alike made one after only where sets apart
/// may give them, as of a column grouped by a collation written.
#[test]
fn distinct_removes_sets_alike_7_13() {
    let (_db, a) = sales();
    let twice = ["NULL,1", "NULL,1", "n,2", "n,2", "s,2", "s,2"];
    assert_eq!(of(&a, "SELECT R, COUNT(*) FROM S GROUP BY ROLLUP (R), R"), twice);
    assert_eq!(of(&a, "SELECT R, COUNT(*) FROM S GROUP BY ALL ROLLUP (R), R"), twice);
    assert_eq!(of(&a, "SELECT R, COUNT(*) FROM S GROUP BY DISTINCT ROLLUP (R), R"), ["NULL,1", "n,2", "s,2"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM S GROUP BY GROUPING SETS ((), ())"), ["5", "5"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM S GROUP BY DISTINCT GROUPING SETS ((), ())"), ["5"]);
    let reordered = "SELECT R, Y, COUNT(*) FROM S GROUP BY DISTINCT GROUPING SETS ((R, Y), (Y, R, Y))";
    assert_eq!(of(&a, reordered).len(), 4, "a set's columns in any order, or twice, are the set");
    let explained = |sql: &str| a.explain(sql, &Params::new()).unwrap();
    let alike = explained("SELECT R, COUNT(*) FROM S GROUP BY DISTINCT ROLLUP (R), R");
    assert!(!alike.contains("Distinct"), "sets alike are one before any row is grouped: {alike}");
    let collated = explained("SELECT R, COUNT(*) FROM S GROUP BY DISTINCT ROLLUP (R COLLATE UCS_BASIC)");
    assert!(collated.contains("Distinct"), "rows alike of sets apart are one after: {collated}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 GR2a, SR10, SR11] the
/// empty grouping set groups T as its only group, even when T is empty:
/// the total of no rows is one row, COUNT 0, SUM null, of a ROLLUP, a CUBE,
/// GROUP BY () and each of two sets alike; a set of columns gives no
/// group of no rows, and HAVING removes the total it rejects. A simple
/// GROUP BY loses its parentheses, so GROUP BY (R), () groups by R alone,
/// and GROUPING is 0 in it, as GROUP BY DISTINCT R is GROUP BY R.
#[test]
fn the_empty_set_groups_even_no_rows_7_13() {
    let (_db, a) = sales();
    assert_eq!(of(&a, "SELECT R, COUNT(*), SUM(AMOUNT), GROUPING(R) FROM E GROUP BY ROLLUP (R)"), ["NULL,0,NULL,1"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM E GROUP BY CUBE (R, Y)"), ["0"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM E GROUP BY ()"), ["0"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM E GROUP BY GROUPING SETS ((), ())"), ["0", "0"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM E GROUP BY DISTINCT GROUPING SETS ((), ())"), ["0"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM E GROUP BY R"), Vec::<String>::new());
    assert_eq!(of(&a, "SELECT COUNT(*) FROM E GROUP BY ROLLUP (R) HAVING GROUPING(R) = 0"), Vec::<String>::new());
    assert_eq!(of(&a, "SELECT COUNT(*) FROM E GROUP BY ROLLUP (R) HAVING COUNT(*) > 0"), Vec::<String>::new());
    let primitive = ["NULL,0,1", "n,0,2", "s,0,2"];
    assert_eq!(of(&a, "SELECT R, GROUPING(R), COUNT(*) FROM S GROUP BY R"), primitive);
    assert_eq!(of(&a, "SELECT R, GROUPING(R), COUNT(*) FROM S GROUP BY (R), ()"), primitive);
    assert_eq!(of(&a, "SELECT R, GROUPING(R), COUNT(*) FROM S GROUP BY DISTINCT R"), primitive);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM S GROUP BY (), ()"), ["5"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR17 m iii 1 A II] a
/// column a set leaves out is null in that set everywhere, in a set
/// function's argument too: the total of a ROLLUP of Y sums no Y, counts
/// no Y, and takes no greatest R of a ROLLUP of R; COUNT(*) still counts
/// its rows, and a column grouped by no set is summed as it is.
#[test]
fn left_out_columns_are_null_in_arguments_7_13() {
    let (_db, a) = sales();
    let summed = "SELECT Y, SUM(Y), COUNT(Y), COUNT(*), SUM(AMOUNT), GROUPING(Y) FROM S GROUP BY ROLLUP (Y)";
    assert_eq!(of(&a, summed), ["1,3,3,3,22,0", "2,4,2,2,21,0", "NULL,NULL,0,5,43,1"]);
    assert_eq!(of(&a, "SELECT MAX(R), COUNT(DISTINCT R) FROM S GROUP BY ROLLUP (R)"), ["NULL,0", "NULL,0", "n,1", "s,1"]);
    assert_eq!(of(&a, "SELECT SUM(Y + AMOUNT) FROM S GROUP BY GROUPING SETS ((Y), ())"), ["25", "25", "NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR17 m iii, §6.9 SR3,
/// SR6b, §7.14] HAVING judges each set's groups, GROUPING and the null of a
/// column left out among them; GROUPING in a subquery of only outer
/// references is the enclosing query's, as an outer reference to a column
/// it groups by is that column's value in the group's set, in the select
/// list and in HAVING. SELECT DISTINCT and ORDER BY apply to every set's
/// rows.
#[test]
fn having_and_subqueries_see_each_set_7_13() {
    let (_db, a) = sales();
    assert_eq!(of(&a, "SELECT R, SUM(AMOUNT) FROM S GROUP BY ROLLUP (R) HAVING GROUPING(R) = 1"), ["NULL,43"]);
    assert_eq!(of(&a, "SELECT R, SUM(AMOUNT) FROM S GROUP BY ROLLUP (R) HAVING R = 'n'"), ["n,30"]);
    let outer = "SELECT R, (SELECT GROUPING(S.R) FROM ONE), (SELECT S.R FROM ONE) FROM S GROUP BY ROLLUP (R)";
    assert_eq!(of(&a, outer), ["NULL,0,NULL", "NULL,1,NULL", "n,0,n", "s,0,s"]);
    let judged = "SELECT SUM(AMOUNT) FROM S GROUP BY ROLLUP (R) HAVING (SELECT GROUPING(S.R) FROM ONE) = 1";
    assert_eq!(of(&a, judged), ["43"]);
    assert_eq!(of(&a, "SELECT DISTINCT GROUPING(R) FROM S GROUP BY ROLLUP (R)"), ["0", "1"]);
    let ordered = "SELECT R, SUM(AMOUNT) AS T FROM S GROUP BY ROLLUP (R) ORDER BY T";
    assert_eq!(rows(&a, ordered).unwrap(), ["NULL,1", "s,12", "n,30", "NULL,43"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR3, SR17n, §9.15] a
/// grouping column groups by its collation, or by the one its `<collate
/// clause>` names; a column grouped by under two collations is a column of
/// each set by the collation that set has. GROUP BY DISTINCT's UNION
/// DISTINCT compares the column by its own collation, so rows alike by it
/// are one, of two such sets or of one set grouped by a finer collation.
#[test]
fn grouping_columns_group_by_their_collations_7_13() {
    let (_db, a) = sales();
    a.run("CREATE TABLE C (P VARCHAR(5) COLLATE CASE_INSENSITIVE, N INTEGER)").unwrap();
    a.run("INSERT INTO C VALUES ('a', 1), ('A', 2), ('b', 3)").unwrap();
    assert_eq!(of(&a, "SELECT COUNT(*), GROUPING(P) FROM C GROUP BY ROLLUP (P)"), ["1,0", "2,0", "3,1"]);
    assert_eq!(of(&a, "SELECT COUNT(*) FROM C GROUP BY ROLLUP (P COLLATE UCS_BASIC)"), ["1", "1", "1", "3"]);
    let both = "SELECT COUNT(*), GROUPING(P) FROM C GROUP BY GROUPING SETS ((P), (P COLLATE UCS_BASIC))";
    assert_eq!(of(&a, both), ["1,0", "1,0", "1,0", "1,0", "2,0"]);
    let once = "SELECT COUNT(*), GROUPING(P) FROM C GROUP BY DISTINCT GROUPING SETS ((P), (P COLLATE UCS_BASIC))";
    assert_eq!(of(&a, once), ["1,0", "1,0", "2,0"]);
    let finer = "SELECT COUNT(*) FROM C GROUP BY DISTINCT ROLLUP (P COLLATE UCS_BASIC)";
    assert_eq!(of(&a, finer), ["1", "1", "3"], "'a' and 'A', each once, are alike by P's own collation");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13, §11.32] a view of
/// grouping sets gives their rows, its column of a column a set leaves
/// out nullable though the column is NOT NULL; a prepared statement's
/// parameter in WHERE is the value given; EXPLAIN shows the groups made of
/// no rows.
#[test]
fn views_and_prepared_statements_of_grouping_sets_7_13() {
    let (_db, a) = sales();
    a.run("CREATE VIEW V (R, T) AS SELECT R, SUM(AMOUNT) FROM S GROUP BY ROLLUP (R)").unwrap();
    assert_eq!(of(&a, "SELECT * FROM V WHERE R IS NULL"), ["NULL,1", "NULL,43"]);
    a.run("CREATE TABLE NN (K INTEGER NOT NULL, J INTEGER NOT NULL)").unwrap();
    a.run("CREATE VIEW W AS SELECT K, J FROM NN GROUP BY GROUPING SETS ((K, J), (K))").unwrap();
    let nullable = "SELECT COLUMN_NAME, IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'W'";
    assert_eq!(of(&a, nullable), ["J,YES", "K,NO"]);
    let p = a.prepare("SELECT R, SUM(AMOUNT) FROM S WHERE AMOUNT > ? GROUP BY CUBE (R)").unwrap();
    let mut cursor = a.open_prepared(&p, &[Value::Exact(Decimal::new(6, 0))]).unwrap();
    let mut count = 0;
    while cursor.fetch().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 3, "n, s and the total of the sales over 6");
    let plan = a.explain("SELECT R, COUNT(*) FROM S GROUP BY ROLLUP (R)", &Params::new()).unwrap();
    assert!(plan.contains("with 1 groups of no rows"), "{plan}");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.9 SR1, SR7, SR10a, SR10b,
/// §7.13 SR1, SR17n, §9.12 SR3] GROUPING of a column no grouping column of
/// its query, of a query not grouped, or with more column references than
/// its type's precision holds, is refused, as is GROUPING in WHERE or in a
/// set function's argument; GROUP BY DISTINCT of a result column of an
/// array type, which no UNION DISTINCT compares; a GROUP BY of more grouping
/// sets than the limit; and ROLLUP of no set, which is no SQL.
#[test]
fn grouping_misused_is_refused_7_13() {
    let (_db, a) = sales();
    let many = vec!["R"; 60].join(", ");
    for (refused, code, rule) in [
        ("SELECT GROUPING(AMOUNT) FROM S GROUP BY ROLLUP (R)", Code::SyntaxRule, "§6.9 SR10a"),
        ("SELECT GROUPING(R) FROM S GROUP BY Y", Code::SyntaxRule, "§6.9 SR10a"),
        ("SELECT GROUPING(R) FROM S", Code::SyntaxRule, "§6.9 SR10a"),
        ("SELECT COUNT(*), GROUPING(R) FROM S GROUP BY ()", Code::SyntaxRule, "§6.9 SR10a"),
        ("SELECT (SELECT GROUPING(S.R) FROM ONE) FROM S", Code::SyntaxRule, "§6.9 SR10a"),
        ("SELECT R, (SELECT GROUPING(S.R, ONE.X) FROM ONE) FROM S GROUP BY ROLLUP (R)", Code::SyntaxRule, "§6.9 SR10a"),
        ("SELECT R FROM S WHERE GROUPING(R) = 0 GROUP BY ROLLUP (R)", Code::SyntaxRule, "§5.21 SR3"),
        ("SELECT SUM(GROUPING(R)) FROM S GROUP BY ROLLUP (R)", Code::SyntaxRule, "§6.5 SR4"),
        ("SELECT AMOUNT FROM S GROUP BY ROLLUP (R)", Code::SyntaxRule, "§5.25 SR7"),
        ("SELECT R, ARRAY_AGG(Y) FROM S GROUP BY DISTINCT ROLLUP (R)", Code::SyntaxRule, "§9.12 SR3"),
    ] {
        let e = fetched(&a, refused).expect_err(refused);
        assert_eq!(e.code, code, "{refused}");
        let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
        assert!(cited.contains(rule), "{refused}: {cited}");
    }
    let wide = format!("SELECT GROUPING({many}) FROM S GROUP BY ROLLUP (R)");
    let cubed = format!("SELECT COUNT(*) FROM S GROUP BY CUBE ({})", vec!["R"; 13].join(", "));
    for (refused, rule) in [(wide.as_str(), "§6.9 SR10b"), (cubed.as_str(), "§7.13 SR13, SR14, SR16")] {
        let e = fetched(&a, refused).expect_err(refused);
        let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
        assert!(e.code == Code::Unsupported && cited.contains(rule), "{cited}");
    }
    assert_eq!(of(&a, "SELECT R, ARRAY_AGG(Y) FROM S GROUP BY ROLLUP (R)").len(), 4, "without DISTINCT an array is no matter");
    assert_eq!(of(&a, "SELECT R, ARRAY_AGG(Y) FROM S GROUP BY DISTINCT R").len(), 3, "nor of a primitive GROUP BY [SR11]");
    assert_eq!(of(&a, &format!("SELECT GROUPING({}) FROM S GROUP BY ROLLUP (R)", vec!["R"; 59].join(", "))).len(), 4);
    for syntax in ["SELECT COUNT(*) FROM S GROUP BY ROLLUP ()", "SELECT COUNT(*) FROM S GROUP BY GROUPING SETS ()"] {
        assert_eq!(rows(&a, syntax).err(), Some(Code::Syntax), "{syntax}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2] ROLLUP, CUBE and
/// GROUPING, which SQL-92 does not reserve, and SETS, which neither
/// reserves, name columns: grouped by, in ROLLUP and as GROUPING's
/// arguments.
#[test]
fn grouping_words_name_columns_7_13() {
    let (_db, a) = sales();
    a.run("CREATE TABLE K (ROLLUP INTEGER, CUBE INTEGER, GROUPING INTEGER, SETS INTEGER)").unwrap();
    a.run("INSERT INTO K VALUES (1, 2, 3, 4)").unwrap();
    assert_eq!(of(&a, "SELECT ROLLUP, CUBE, GROUPING, SETS FROM K GROUP BY ROLLUP, CUBE, GROUPING, SETS"), ["1,2,3,4"]);
    let rolled = "SELECT ROLLUP, GROUPING(ROLLUP), GROUPING(GROUPING, SETS) FROM K GROUP BY ROLLUP (ROLLUP, GROUPING, SETS)";
    assert_eq!(of(&a, rolled), ["1,0,0", "1,0,1", "1,0,3", "NULL,1,3"]);
    a.rollback().unwrap();
}

/// Schema A: S, of 3000 sales of 10 products or none, in 100 days.
fn many() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE S (P INTEGER, D INTEGER, AMOUNT INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0..3000).step_by(500) {
        let values: Vec<String> = (first..first + 500)
            .map(|i| {
                let product = if i % 11 == 10 { "NULL".to_owned() } else { (i % 11).to_string() };
                format!("({product}, {}, {})", i % 100, i % 7)
            })
            .collect();
        a.run(&format!("INSERT INTO S VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    (db, a)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR17n, SR17o] [DESIGN]
/// RULES.md D38: a ROLLUP and a CUBE give what the UNION ALL of one GROUP
/// BY for each set gives, in memory, and, with MEMORY.WORK_BYTES at its
/// least, past the work area, the total's group made before any row
/// kept whatever is spilled.
#[test]
fn grouping_sets_answer_as_defined_7_13() {
    let (db, a) = many();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let null = "CAST(NULL AS INTEGER)";
    let cases = [
        (
            "SELECT P, D, COUNT(*), SUM(AMOUNT), GROUPING(P, D) FROM S GROUP BY ROLLUP (P, D)".to_owned(),
            format!(
                "SELECT P, D, COUNT(*), SUM(AMOUNT), 0 FROM S GROUP BY P, D UNION ALL \
                 SELECT P, {null}, COUNT(*), SUM(AMOUNT), 1 FROM S GROUP BY P UNION ALL \
                 SELECT {null}, {null}, COUNT(*), SUM(AMOUNT), 3 FROM S"
            ),
        ),
        (
            "SELECT P, D, COUNT(*), MAX(D) FROM S GROUP BY CUBE (P, D)".to_owned(),
            format!(
                "SELECT P, D, COUNT(*), MAX(D) FROM S GROUP BY P, D UNION ALL \
                 SELECT P, {null}, COUNT(*), {null} FROM S GROUP BY P UNION ALL \
                 SELECT {null}, D, COUNT(*), MAX(D) FROM S GROUP BY D UNION ALL \
                 SELECT {null}, {null}, COUNT(*), {null} FROM S"
            ),
        ),
    ];
    for (sql, defined) in &cases {
        assert_eq!(of(&a, sql), of(&a, defined), "{sql}");
    }
    assert_eq!(counted.taken(), 0, "the default MEMORY.WORK_BYTES holds them");
    let in_memory: Vec<Vec<String>> = cases.iter().map(|(_, defined)| of(&a, defined)).collect();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for ((sql, _), expected) in cases.iter().zip(&in_memory) {
        assert_eq!(&of(&a, sql), expected, "{sql}, spilled");
    }
    assert!(counted.taken() > 0, "they spilled");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 GR2a, §11.34 GR2]
/// [DESIGN] RULES.md D51: an assertion of grouping sets is judged whole,
/// the total's group of every row, not of the rows of the groups changed:
/// two rows, each of a group under the bound, break it by their total. Of
/// a grouping by a column alone the groups changed are judged, as before.
///
/// Decision `!collations.is_empty() || !seeds.is_empty()` in `grouped`:
/// seeds alone (judged whole) against neither (groups changed).
#[test]
fn assertions_of_grouping_sets_judge_the_total_7_13() {
    let (_db, a) = sales();
    a.run("CREATE TABLE G (K INTEGER, V INTEGER)").unwrap();
    a.run("CREATE ASSERTION TOTAL CHECK (NOT EXISTS (SELECT K FROM G GROUP BY ROLLUP (K) HAVING SUM(V) > 10))").unwrap();
    a.run("CREATE ASSERTION EACH CHECK (NOT EXISTS (SELECT K FROM G GROUP BY K HAVING SUM(V) > 8))").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO G VALUES (1, 6)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO G VALUES (2, 6)"), Err(Code::Constraint), "a total of 12");
    assert_eq!(exec(&a, "INSERT INTO G VALUES (1, 3)"), Err(Code::Constraint), "a group of 9");
    a.run("INSERT INTO G VALUES (2, 4)").unwrap();
    assert_eq!(of(&a, "SELECT SUM(V) FROM G"), ["10"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 GR2a, §7.17 GR] a
/// recursive query whose recursive operand groups by grouping sets with an
/// empty one is evaluated whole, not by the rows each iteration adds: of
/// no rows the empty set's group is still a row, so a recursion whose
/// anchor gives none gives that row.
#[test]
fn recursive_grouping_sets_group_no_rows_7_13() {
    let (_db, a) = sales();
    let empty_anchor = "WITH RECURSIVE R (N) AS (SELECT X FROM ONE WHERE X > 5 UNION \
                        SELECT N FROM R GROUP BY GROUPING SETS ((N), ())) SELECT * FROM R";
    assert_eq!(of(&a, empty_anchor), ["NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 GR2a] [DESIGN] the groups
/// of grouping sets are put in order as ORDER BY asks, the total's group,
/// made before any row, among them: it is not taken to come in the order
/// of the grouping columns, as groups of their rows are.
///
/// Decision `collations.is_empty() && seeds.is_empty()` in the planner:
/// seeds (sorted after grouping) against neither (in the grouping's order).
#[test]
fn grouping_sets_are_ordered_as_asked_7_13() {
    let (_db, a) = sales();
    let ordered = "SELECT Y, COUNT(*) FROM S GROUP BY ROLLUP (Y) ORDER BY Y";
    assert_eq!(rows(&a, ordered).unwrap(), ["1,3", "2,2", "NULL,5"]);
    let descending = "SELECT Y, COUNT(*) FROM S GROUP BY ROLLUP (Y) ORDER BY Y DESC";
    assert_eq!(rows(&a, descending).unwrap(), ["NULL,5", "2,2", "1,3"]);
    assert_eq!(rows(&a, "SELECT Y, COUNT(*) FROM S GROUP BY Y ORDER BY Y").unwrap(), ["1,3", "2,2"]);
}
