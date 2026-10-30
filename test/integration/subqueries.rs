//! The `<subquery>` [X3.135-1986 §5.24] and the predicates that have one:
//! the `<comparison predicate>` [§5.11], the `<in predicate>` [§5.13], the
//! `<quantified predicate>` [§5.16] and the `<exists predicate>` [§5.17];
//! outer references [§5.7 SR3, SR4], and subqueries of HAVING clauses
//! [§5.23] and of searched DELETE and UPDATE [§8.5, §8.12].

use turborel::db::{Completion, Database};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Value};

use crate::common::{code, exec, expected, rows, setup};
use turborel::value::Spelled;

/// Verifies: [X3.135-1986 §5.13 GR2, §5.16 GR2] "x IN S" is "x =ANY S",
/// "x NOT IN S" its negation: so a null in S and no match makes both
/// unknown; the types are comparable [§5.13 SR1]; a subquery other than
/// under EXISTS has one column, so "*" needs a table of degree 1 [§5.24
/// SR2a].
#[test]
fn in_a_subquery_is_equal_to_any_5_13() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID IN (SELECT ID FROM U)"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID NOT IN (SELECT ID FROM U) ORDER BY ID"), expected(&["2", "3"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID NOT IN (SELECT AMT FROM T)"), expected(&[]), "row 3's AMT is null");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID IN (SELECT AMT FROM T)"), expected(&[]));
    assert_eq!(rows(&s, "SELECT DISTINCT ID FROM T WHERE ID IN (SELECT DISTINCT ID FROM U)"), expected(&["1"]), "[SR8] its own DISTINCT");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID IN (SELECT CODE FROM U)"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID IN (SELECT * FROM U)"), Code::SyntaxRule, "degree 2");
    assert_eq!(
        rows(&s, "SELECT ID FROM T WHERE ID IN (SELECT DISTINCT COUNT(DISTINCT ID) FROM U)"),
        expected(&["1"]),
        "two DISTINCTs [ISO/IEC 9075:1992 §7.9, Leveling Rules]"
    );
    assert_eq!(
        code(&s, "SELECT ID FROM T WHERE ID IN (SELECT ID, CODE FROM U)"),
        Code::SyntaxRule,
        "degree 2 [ISO/IEC 9075:1992 §8.7 SR1]"
    );
    db.schema("CREATE SCHEMA AUTHORIZATION K CREATE TABLE ONE (X INTEGER) CREATE TABLE TWO (Y INTEGER)").unwrap();
    let k = db.session("K");
    k.run("INSERT INTO ONE VALUES (2)").unwrap();
    k.run("INSERT INTO TWO VALUES (2)").unwrap();
    assert_eq!(rows(&k, "SELECT Y FROM TWO WHERE Y IN (SELECT * FROM ONE)"), expected(&["2"]), "degree 1");
    assert_eq!(code(&k, "SELECT Y FROM TWO WHERE Y IN (SELECT ID FROM APP.T)"), Code::Privilege, "[SR1]");
}

/// Verifies: [X3.135-1986 §5.17 GR3] EXISTS is true if and only if the
/// subquery's result is not empty, never unknown; its "*" is any value
/// [§5.24 SR2b], also of a grouped table; a column specification is looked
/// up in the most local scope first, and outward only if no table there has
/// the column [§5.7 SR3], an outer reference then being the value in the
/// row the predicate is applied to [§5.21 GR3].
#[test]
fn exists_and_outer_references_5_17() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE U.ID = T.ID)"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT EXISTS (SELECT * FROM U WHERE U.ID = T.ID) ORDER BY ID"), expected(&["2", "3"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE EXISTS (SELECT CODE FROM U WHERE CODE = 'Z')"), expected(&[]));
    // ID is U's, the most local: every row of T qualifies.
    assert_eq!(
        rows(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE CODE = 'B' AND U.ID = ID) ORDER BY ID"),
        expected(&["1", "2", "3"])
    );
    // NAME is T's, the only table with one.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE NAME = 'ANN')"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T X WHERE EXISTS (SELECT * FROM T WHERE T.ID = X.ID + 1) ORDER BY ID"), expected(&["1", "2"]));
    let grouped = "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U GROUP BY ID HAVING COUNT(*) > 1) ORDER BY ID";
    assert_eq!(rows(&s, grouped), expected(&["1", "2", "3"]));
    assert_eq!(code(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE U.NAME = 'A')"), Code::SyntaxRule, "U has no NAME");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE NOPE = 1)"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE Q.ID = 1)"), Code::SyntaxRule);
    let plan = s.explain("SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE U.ID = T.ID)", &Params::new()).unwrap();
    assert!(plan.contains("Subquery") && plan.contains("U.ID = T.ID"), "{plan}");
}

/// Verifies: [X3.135-1986 §5.11 GR1, GR2] a comparison with a subquery
/// takes its one value; an empty result makes it unknown; more than one
/// value is a cardinality violation; the subquery is not grouped [§5.24
/// SR9]; an error raised running it is the statement's.
#[test]
fn comparison_with_a_subquery_5_11() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = (SELECT MAX(ID) FROM U)"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE AMT > (SELECT AMT FROM T WHERE ID = 2)"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T X WHERE ID = (SELECT ID FROM T WHERE ID = X.ID) ORDER BY ID"), expected(&["1", "2", "3"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = (SELECT ID FROM U WHERE CODE = 'Z')"), expected(&[]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT (ID = (SELECT ID FROM U WHERE CODE = 'Z'))"), expected(&[]), "unknown");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID = (SELECT ID FROM U)"), Code::Cardinality);
    // [ISO/IEC 9075:1992 §7.11, Leveling Rules] grouped: Entry SQL refuses it, SQL-92 does not.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = (SELECT ID FROM U GROUP BY ID)"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = (SELECT COUNT(*) FROM U HAVING COUNT(*) > 0)"), expected(&["2"]));
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID = (SELECT 1 / 0 FROM U WHERE CODE = 'A')"), Code::Data);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID = (SELECT CODE FROM U WHERE CODE = 'A')"), Code::SyntaxRule);
    let mut p = Params::new().declare("N", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null).declare(
        "C",
        DataType::Character { length: 3 },
        Value::Char("B".into()),
    );
    s.execute("SELECT COUNT(*) INTO N FROM T WHERE ID = (SELECT ID FROM U WHERE CODE = C)", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "1", "a parameter in the subquery");
}

/// Verifies: [X3.135-1986 §5.16 GR2] "x op ALL S" is true if S is empty or
/// "x op s" is true for every s, false if it is false for one, else
/// unknown; "x op SOME S" (or ANY) is true if it is true for one, false if
/// S is empty or it is false for every s, else unknown; the types are
/// comparable [SR1].
#[test]
fn quantified_comparisons_5_16() {
    let db = setup();
    let s = db.session("APP");
    let q = |sql: &str| rows(&s, &format!("SELECT ID FROM T WHERE {sql} ORDER BY ID"));
    assert_eq!(q("ID > ALL (SELECT ID FROM U)"), expected(&["2", "3"]));
    assert_eq!(q("ID = ANY (SELECT ID FROM U)"), expected(&["1"]));
    assert_eq!(q("ID = SOME (SELECT ID FROM U)"), expected(&["1"]));
    assert_eq!(q("ID > ALL (SELECT ID FROM U WHERE CODE = 'Z')"), expected(&["1", "2", "3"]), "empty: true");
    assert_eq!(q("NOT (ID = ANY (SELECT ID FROM U WHERE CODE = 'Z'))"), expected(&["1", "2", "3"]), "empty: false");
    assert_eq!(q("ID < ALL (SELECT AMT FROM T)"), expected(&[]), "false for -3.00");
    assert_eq!(q("ID <> ALL (SELECT AMT FROM T)"), expected(&[]), "unknown for the null");
    assert_eq!(q("NOT (ID <> ALL (SELECT AMT FROM T))"), expected(&[]));
    assert_eq!(q("ID > ANY (SELECT AMT FROM T)"), expected(&["1", "2", "3"]), "true for -3.00");
    assert_eq!(q("AMT = ANY (SELECT AMT FROM T)"), expected(&["1", "2"]), "row 3's AMT is null");
    assert_eq!(q("NOT (AMT = ANY (SELECT AMT FROM T))"), expected(&[]));
    assert_eq!(q("ID >= ALL (SELECT X.ID FROM T X WHERE X.AMT < T.AMT)"), expected(&["2", "3"]), "correlated");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID = ALL (SELECT CODE FROM U)"), Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §5.23 GR4, SR2] a subquery of a HAVING clause is
/// run for each group, its outer references being to grouping columns or
/// in set functions; [§5.23 GR3, §5.8 SR4, SR5, §5.21 SR3] a set function
/// over an outer reference has the outer group as its argument source, in
/// the subquery's select list or WHERE clause, nested deeper too; it is not
/// in a subquery of a WHERE clause. [ISO/IEC 9075:1992 §7.9 GR1a] It does
/// not make the subquery aggregated, as SQL-86 has it [§5.24 GR1]: the
/// subquery gives a row for each of its own.
#[test]
fn subqueries_of_having_clauses_5_23() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T GROUP BY ID HAVING ID IN (SELECT ID FROM U)"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID, COUNT(*) FROM T GROUP BY ID HAVING EXISTS (SELECT * FROM U WHERE U.ID = T.ID)"), expected(&["1,1"]));
    assert_eq!(code(&s, "SELECT ID FROM T GROUP BY ID HAVING EXISTS (SELECT * FROM U WHERE U.ID = T.AMT)"), Code::SyntaxRule);
    assert_eq!(
        rows(&s, "SELECT ID FROM T GROUP BY ID HAVING 1 < (SELECT MAX(T.ID) FROM U WHERE CODE = 'A') ORDER BY ID"),
        expected(&["2", "3"])
    );
    let sql = "SELECT NAME FROM T GROUP BY NAME HAVING EXISTS (SELECT * FROM U WHERE U.ID = MAX(T.ID))";
    assert_eq!(rows(&s, sql), expected(&["ANN   "]));
    let sql = "SELECT ID FROM T GROUP BY ID HAVING EXISTS (SELECT * FROM U WHERE EXISTS (SELECT * FROM U X WHERE X.ID = MAX(T.ID)))";
    assert_eq!(rows(&s, sql), expected(&["1"]), "nested");
    let sql = "SELECT COUNT(*) FROM T HAVING 2 = (SELECT COUNT(DISTINCT T.NAME) FROM U WHERE CODE = 'A')";
    assert_eq!(rows(&s, sql), expected(&["3"]), "no GROUP BY: one group");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE U.ID = MAX(T.ID))"), Code::SyntaxRule, "SR5");
    assert_eq!(code(&s, "SELECT ID FROM T GROUP BY ID HAVING EXISTS (SELECT * FROM U WHERE MAX(U.ID) = 1)"), Code::SyntaxRule, "§5.21 SR3");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 SR4, SR5, §7.9 SR7]: the
/// argument of a set function over an outer reference is any expression
/// whose only column reference it is, SQL-86's rule of no operators
/// [X3.135-1986 §5.8 SR4] being Entry SQL's; it may be in a subquery of
/// the select list of the query of its column, which is then aggregated,
/// one group of its rows, every other column reference in its select list
/// to be in a set function; it does not make the subquery aggregated.
#[test]
fn outer_set_functions_of_expressions_6_5() {
    let db = setup();
    let s = db.session("APP");
    let sql = "SELECT ID FROM T GROUP BY ID HAVING 2 < (SELECT MAX(T.ID * 2) FROM U WHERE CODE = 'A') ORDER BY ID";
    assert_eq!(rows(&s, sql), expected(&["2", "3"]));
    let sql = "SELECT ID, (SELECT SUM(T.AMT * 2) FROM U WHERE CODE = 'A') FROM T GROUP BY ID ORDER BY ID";
    assert_eq!(rows(&s, sql), expected(&["1,21.00", "2,-6.00", "3,NULL"]));
    let sql = "SELECT (SELECT SUM(T.AMT) FROM U WHERE CODE = 'A') FROM T";
    assert_eq!(rows(&s, sql), expected(&["7.50"]), "one group of T's rows");
    assert_eq!(rows(&s, "SELECT (SELECT SUM(T.AMT) FROM U WHERE CODE = 'A') FROM T WHERE ID > 9"), expected(&["NULL"]));
    let sql = "SELECT (SELECT COUNT(*) FROM U WHERE U.ID < COUNT(DISTINCT T.ID + 0)), COUNT(*) FROM T";
    assert_eq!(rows(&s, sql), expected(&["2,3"]), "in the subquery's WHERE clause, beside its own");
    assert_eq!(code(&s, "SELECT (SELECT SUM(T.AMT) FROM U) FROM T"), Code::Cardinality, "U is not aggregated by it: two rows");
    assert_eq!(code(&s, "SELECT ID, (SELECT SUM(T.AMT) FROM U WHERE CODE = 'A') FROM T"), Code::SyntaxRule, "§7.9 SR7");
    assert_eq!(code(&s, "SELECT ID FROM T GROUP BY ID HAVING 1 < (SELECT MAX(T.ID + U.ID) FROM U)"), Code::SyntaxRule, "SR4");
    assert_eq!(code(&s, "SELECT ID FROM T GROUP BY ID HAVING 1 < (SELECT MAX(T.ID + T.ID) FROM U)"), Code::SyntaxRule, "SR4: the only one");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE 1 < (SELECT MAX(T.ID * 2) FROM U)"), Code::SyntaxRule, "SR5");
}

/// Verifies: [X3.135-1986 §5.7 SR4] an outer reference may be to any
/// enclosing query, not only the next: a subquery's subquery may reference
/// the outermost row.
#[test]
fn nested_subqueries_5_7() {
    let db = setup();
    let s = db.session("APP");
    let sql = "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE EXISTS (SELECT * FROM T X WHERE X.ID = U.ID AND X.ID = T.ID))";
    assert_eq!(rows(&s, sql), expected(&["1"]));
    let sql = "SELECT ID FROM T WHERE ID IN (SELECT ID FROM U WHERE CODE IN (SELECT CODE FROM U WHERE CODE > 'A'))";
    assert_eq!(rows(&s, sql), expected(&["1"]));
}

/// Verifies: [X3.135-1986 §5.21 GR3] a correlated subquery is run with the
/// values of the given row, wherever its outer references are: in a
/// product's conditions, under operators, more than once, and in its
/// select list.
#[test]
fn correlated_subqueries_see_the_given_row_5_21() {
    let db = setup();
    let s = db.session("APP");
    let sql = "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U, T X WHERE X.ID = U.ID AND U.ID = T.ID AND -U.ID = -T.ID \
               AND (X.NAME LIKE 'A%' OR T.NAME IS NULL) AND NOT (U.CODE = 'Z'))";
    assert_eq!(rows(&s, sql), expected(&["1"]));
    let sql = "SELECT ID FROM T WHERE 2 = (SELECT COUNT(*) FROM U WHERE U.ID = T.ID AND U.ID * 2 = T.ID + T.ID)";
    assert_eq!(rows(&s, sql), expected(&["1"]));
    let sql = "SELECT ID FROM T WHERE ID + 1 IN (SELECT T.ID + U.ID FROM U) ORDER BY ID";
    assert_eq!(rows(&s, sql), expected(&["1", "2", "3"]), "every row: T.ID + 1 = T.ID + U.ID");
}

/// Verifies: [X3.135-1986 §8.5 GR1b, §8.12 GR1] a searched DELETE or
/// UPDATE runs its subqueries for each row of T, an outer reference to T
/// being to that row. [ISO/IEC 9075:1992 §13.7 GR2b, §13.10 GR2b, GR6]
/// They may read T, however deeply nested: SQL-86's rule that they not
/// [§8.5 SR2, §8.12 SR2] is Intermediate SQL's [§13.7, §13.10, Leveling
/// Rules]; the condition, and every SET value, are evaluated for every row
/// before any row is changed.
#[test]
fn searched_changes_with_subqueries_8_5_8_12() {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "UPDATE T SET AMT = 0 WHERE EXISTS (SELECT * FROM U WHERE U.ID = T.ID AND CODE = 'B')").unwrap();
    assert_eq!(rows(&s, "SELECT ID, AMT FROM T WHERE AMT = 0"), expected(&["1,0.00"]));
    exec(&s, "UPDATE T SET AMT = (SELECT MAX(AMT) FROM T) + ID").unwrap();
    assert_eq!(
        rows(&s, "SELECT ID, AMT FROM T ORDER BY ID"),
        expected(&["1,1.00", "2,2.00", "3,3.00"]),
        "GR6: MAX(AMT) 0 for each, not 1, 3, 6"
    );
    exec(&s, "UPDATE T SET AMT = 0 WHERE AMT < (SELECT MAX(AMT) FROM T)").unwrap();
    assert_eq!(rows(&s, "SELECT ID, AMT FROM T ORDER BY ID"), expected(&["1,0.00", "2,0.00", "3,3.00"]), "GR2b");
    exec(&s, "DELETE FROM T WHERE ID = (SELECT MIN(ID) FROM T)").unwrap();
    assert_eq!(rows(&s, "SELECT ID FROM T ORDER BY ID"), expected(&["2", "3"]), "§13.7 GR2b: MIN once, before any deletion");
    exec(&s, "DELETE FROM U WHERE EXISTS (SELECT * FROM T WHERE EXISTS (SELECT * FROM U X WHERE X.CODE = 'B'))").unwrap();
    assert_eq!(rows(&s, "SELECT ID FROM U"), expected(&[]));
    exec(&s, "DELETE FROM T WHERE ID NOT IN (SELECT ID FROM U)").unwrap();
    assert_eq!(rows(&s, "SELECT ID FROM T"), expected(&[]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.9 SR12d, §13.1 SR5]: a query
/// specification whose WHERE clause has a subquery is updatable unless the
/// subquery reads its table, so its cursor allows a positioned DELETE
/// [§13.6 SR2], and each row is derived as the cursor reaches it, the
/// subquery run again; one whose subquery reads its table is read-only.
/// SQL-86's rule of no subquery [X3.135-1986 §5.25 SR11d] is SQL-92's
/// SR12d.
#[test]
fn a_subquery_reading_its_table_makes_a_cursor_read_only_7_9() {
    let db = setup();
    let s = db.session("APP");
    let mut p = Params::new().declare("ID_OUT", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    s.run("INSERT INTO U VALUES (2, 'X')").unwrap();
    s.declare_cursor("DECLARE S CURSOR FOR SELECT ID FROM T WHERE ID IN (SELECT ID FROM U)").unwrap();
    s.execute("OPEN S", &mut p).unwrap();
    s.run("DELETE FROM U WHERE ID = 2").unwrap();
    s.execute("FETCH S INTO ID_OUT", &mut p).unwrap();
    assert_eq!(p.value("ID_OUT").to_string(), "1");
    assert_eq!(s.execute("UPDATE T SET AMT = (SELECT COUNT(*) FROM T) WHERE CURRENT OF S", &mut p).unwrap(), Completion::Success);
    assert_eq!(rows(&s, "SELECT AMT FROM T WHERE ID = 1"), expected(&["3.00"]), "§13.9: a SET value's subquery reads T");
    assert_eq!(s.execute("DELETE FROM T WHERE CURRENT OF S", &mut p).unwrap(), Completion::Success);
    assert_eq!(s.execute("FETCH S INTO ID_OUT", &mut p).unwrap(), Completion::NoData, "2 is no longer in U");
    s.declare_cursor("DECLARE R CURSOR FOR SELECT ID FROM T WHERE ID > (SELECT MIN(ID) FROM T)").unwrap();
    s.execute("OPEN R", &mut p).unwrap();
    s.execute("FETCH R INTO ID_OUT", &mut p).unwrap();
    assert_eq!(s.execute("DELETE FROM T WHERE CURRENT OF R", &mut p).unwrap_err().code, Code::SyntaxRule, "SR12d");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.9 SR12d, §11.19 SR6, GR11,
/// GR12]: a view whose WHERE clause has a subquery that reads neither its
/// table nor, through views, that table's base table is updatable, WITH
/// CHECK OPTION running the subquery for each row it changes; one whose
/// subquery does is read-only.
#[test]
fn views_with_subqueries_7_9() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE P (K INTEGER)
           CREATE TABLE OK (K INTEGER)
           CREATE VIEW CHECKED AS SELECT K FROM P WHERE K IN (SELECT K FROM OK) WITH CHECK OPTION
           CREATE VIEW SELF AS SELECT K FROM P WHERE K > (SELECT MIN(K) FROM P)
           CREATE VIEW OVER AS SELECT K FROM CHECKED WHERE K IN (SELECT K FROM CHECKED)",
    )
    .unwrap();
    let s = db.session("Q");
    s.run("INSERT INTO OK VALUES 1, 2").unwrap();
    assert_eq!(exec(&s, "INSERT INTO CHECKED VALUES 1, 2"), Ok(Completion::Success));
    assert_eq!(exec(&s, "INSERT INTO CHECKED VALUES 3"), Err(Code::Constraint), "3 is not in OK");
    assert_eq!(exec(&s, "UPDATE CHECKED SET K = K + 1"), Err(Code::Constraint), "3 is not in OK");
    assert_eq!(exec(&s, "UPDATE CHECKED SET K = K - 1 WHERE K = 2"), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT K FROM P ORDER BY K"), expected(&["1", "1"]));
    assert_eq!(exec(&s, "DELETE FROM SELF"), Err(Code::SyntaxRule), "SR12d");
    assert_eq!(exec(&s, "DELETE FROM OVER"), Err(Code::SyntaxRule), "reads P through CHECKED");
}

/// Verifies: [DESIGN] RULES.md D35, decorrelation (`exec/subquery.rs`): a
/// subquery run for many sets of outer values, its only uses of them
/// equalities with its columns, is run once without them and its rows kept
/// by those columns, and gives what a run per set of values gives
/// [X3.135-1986 §5.21 GR3]: under EXISTS and NOT EXISTS [§5.17], IN
/// [§5.13], ALL [§5.16], DISTINCT, and a comparison whose subquery has more
/// than one row for some values, which fails either way [§5.11 GR1]; nulls
/// on either side match nothing [§5.11 GR2]; an exact key equals an
/// approximate one by value [§5.11 GR4]. The reference wraps the equality
/// as `(... OR 1 = 0)`, which keeps the subquery from being decorrelated;
/// the same whatever runs EXECUTOR.DECORRELATE_AFTER waits for (RULES.md
/// D36).
#[test]
fn decorrelated_subqueries_give_what_each_run_gives_5_21() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE T (K DECIMAL(5,1), V INTEGER)
           CREATE TABLE U (K FLOAT, W INTEGER)",
    )
    .unwrap();
    let s = db.session("Q");
    for k in 0..40 {
        let key = if k % 9 == 0 { "NULL".to_string() } else { format!("{k}.0") };
        s.run(&format!("INSERT INTO T VALUES ({key}, {})", k % 6)).unwrap();
        if k % 3 != 0 {
            s.run(&format!("INSERT INTO U VALUES ({k}E0, {})", k % 5)).unwrap();
        }
        if k % 4 == 0 {
            s.run(&format!("INSERT INTO U VALUES ({k}E0, {})", k % 7)).unwrap();
        }
    }
    s.run("INSERT INTO U VALUES (NULL, 1)").unwrap();
    s.commit().unwrap();
    let queries = [
        "SELECT V FROM T WHERE EXISTS (SELECT * FROM U WHERE U.K = T.K)",
        "SELECT V FROM T WHERE NOT EXISTS (SELECT * FROM U WHERE T.K = U.K AND U.W > 1)",
        "SELECT K FROM T WHERE V IN (SELECT W FROM U WHERE U.K = T.K)",
        "SELECT K FROM T WHERE V NOT IN (SELECT DISTINCT W FROM U WHERE U.K = T.K)",
        "SELECT K FROM T WHERE V >= ALL (SELECT W FROM U WHERE U.K = T.K AND U.W < 4)",
        "SELECT K FROM T WHERE V = (SELECT W FROM U WHERE U.K = T.K)",
    ];
    // Decorrelated after EXECUTOR.DECORRELATE_AFTER runs: by default, at
    // once, and never.
    for (after, sql) in [8, 0, 1_000_000].into_iter().flat_map(|after| queries.map(|q| (after, q))) {
        db.set_setting("EXECUTOR.DECORRELATE_AFTER", &crate::common::number(after)).unwrap();
        let reference = sql.replace("U.K = T.K", "(U.K = T.K OR 1 = 0)").replace("T.K = U.K", "(T.K = U.K OR 1 = 0)");
        let sorted = |q: &str| {
            rows(&s, q).map(|mut r| {
                r.sort();
                r
            })
        };
        assert_eq!(sorted(sql), sorted(&reference), "{sql}, after {after}");
    }
    db.reset_setting("EXECUTOR.DECORRELATE_AFTER").unwrap();
    assert_eq!(code(&s, "SELECT K FROM T WHERE V = (SELECT W FROM U WHERE U.K = T.K)"), Code::Cardinality, "key 4 has two rows");
}

/// Verifies: [X3.135-1986 §5.13 GR2, §5.16 GR2; ISO/IEC 9075:1992 §8.7
/// GR2] IN, NOT IN, and ALL and SOME with each operator, for one value
/// against a subquery's column (answered by a hash of its rows, or by its
/// least and greatest values, RULES.md D38): true, false or unknown as the
/// standard says, with an integer compared with real values, a null among
/// them, none, or a null to compare.
#[test]
fn one_value_is_compared_with_all_or_some_rows_5_16() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE A (X INTEGER)
           CREATE TABLE B (Y REAL)
           CREATE TABLE C (Y REAL)
           CREATE TABLE E (Y REAL)",
    )
    .unwrap();
    let s = db.session("Q");
    for sql in ["INSERT INTO A VALUES (1), (2), (5), (NULL)", "INSERT INTO B VALUES (2E0), (4E0)", "INSERT INTO C VALUES (2E0), (NULL)"] {
        s.run(sql).unwrap();
    }
    // Truth for X = 1, 2, 5, then the null X.
    let truths = |cond: &str| {
        let sql = format!("SELECT X, CASE WHEN {cond} THEN 'T' WHEN NOT ({cond}) THEN 'F' ELSE 'U' END FROM A ORDER BY X");
        // The null X sorts last.
        let found: Vec<String> = rows(&s, &sql).unwrap().iter().filter_map(|r| r.split(',').nth(1).map(str::to_owned)).collect();
        found.concat()
    };
    let cases = [
        ("X IN (SELECT Y FROM B)", "FTFU"),
        ("X IN (SELECT Y FROM C)", "UTUU"),
        ("X IN (SELECT Y FROM E)", "FFFF"),
        ("X NOT IN (SELECT Y FROM C)", "UFUU"),
        ("X NOT IN (SELECT Y FROM B)", "TFTU"),
        ("X < ALL (SELECT Y FROM B)", "TFFU"),
        ("X < ALL (SELECT Y FROM C)", "UFFU"),
        ("X < ALL (SELECT Y FROM E)", "TTTT"),
        ("X <= ALL (SELECT Y FROM B)", "TTFU"),
        ("X >= ALL (SELECT Y FROM C)", "FUUU"),
        ("X > SOME (SELECT Y FROM B)", "FFTU"),
        ("X > SOME (SELECT Y FROM C)", "UUTU"),
        ("X > SOME (SELECT Y FROM E)", "FFFF"),
        ("X >= SOME (SELECT Y FROM B)", "FTTU"),
        ("X < SOME (SELECT Y FROM B)", "TTFU"),
        ("X = ALL (SELECT Y FROM B)", "FFFU"),
        ("X = ALL (SELECT Y FROM B WHERE Y = 2E0)", "FTFU"),
        ("X <> SOME (SELECT Y FROM B)", "TTTU"),
        ("X <> SOME (SELECT Y FROM B WHERE Y = 2E0)", "TFTU"),
        ("X <> SOME (SELECT Y FROM C)", "TUTU"),
    ];
    for (cond, expected) in cases {
        assert_eq!(truths(cond), expected, "{cond}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR7, §8.4 SR3, SR4, §8.7
/// GR2] IN, NOT IN,
/// and ALL and SOME, for a row of two values against a subquery of two
/// columns: true, false or unknown as the standard says, pair by pair, with
/// a null in some of the subquery's rows and in one row compared.
#[test]
fn rows_are_compared_with_all_or_some_rows_8_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE P (A INTEGER, B INTEGER) CREATE TABLE S (C INTEGER, D INTEGER)").unwrap();
    let s = db.session("Q");
    s.run("INSERT INTO P VALUES (1, 1), (2, 5), (3, 3), (NULL, 1)").unwrap();
    s.run("INSERT INTO S VALUES (1, 1), (2, NULL), (4, 4)").unwrap();
    // Truth for (1, 1), (2, 5), (3, 3), then (NULL, 1), which sorts last.
    let truths = |cond: &str| {
        let sql = format!("SELECT A, CASE WHEN {cond} THEN 'T' WHEN NOT ({cond}) THEN 'F' ELSE 'U' END FROM P ORDER BY A");
        rows(&s, &sql).unwrap().iter().filter_map(|r| r.split(',').nth(1).map(str::to_owned)).collect::<String>()
    };
    let cases = [
        ("(A, B) IN (SELECT C, D FROM S)", "TUFU"),
        ("(A, B) NOT IN (SELECT C, D FROM S)", "FUTU"),
        ("(A, B) > ALL (SELECT C, D FROM S WHERE C < 4)", "FUTU"),
        ("(A, B) < SOME (SELECT C, D FROM S WHERE C < 4)", "TUFU"),
        ("(A, B) = ALL (SELECT C, D FROM S WHERE C = 1)", "TFFU"),
        ("(A, B) <> SOME (SELECT C, D FROM S WHERE C = 2)", "TUTU"),
    ];
    for (cond, expected) in cases {
        assert_eq!(truths(cond), expected, "{cond}");
    }
}
