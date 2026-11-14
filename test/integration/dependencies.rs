//! Functional dependencies [ISO/IEC 9075-2:2023 §4.26, Feature T301]: a
//! grouped query names, outside set functions, columns functionally
//! dependent on its grouping columns [§6.7 SR8], in its select list, HAVING
//! and window clause and in its subqueries' outer references,
//! known of base tables' keys and generated columns, of equalities of
//! WHERE and of joins, of views, derived tables and WITH queries, of each
//! grouping set; each value its group's [GR1e], no null eliminated; and
//! each refusal.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{Counted, code, exec, expected, rows};

/// Schema A: DEPT of three departments; STAFF of four people, one of no
/// department, salary or nickname, with a UNIQUE NOT NULL code, a UNIQUE
/// nullable nickname and a generated bonus; LATE, of a deferrable key; and
/// LOOSE, unique of nulls not distinct.
fn company() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE DEPT (ID INTEGER PRIMARY KEY, NAME VARCHAR(20), CITY VARCHAR(20)) \
         CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(20) NOT NULL, DEPT INTEGER, SALARY INTEGER, \
           CODE CHAR(4) NOT NULL UNIQUE, NICK VARCHAR(10) UNIQUE, BONUS INTEGER GENERATED ALWAYS AS (SALARY / 10)) \
         CREATE TABLE LATE (K INTEGER, V INTEGER, CONSTRAINT LATE_KEY PRIMARY KEY (K) DEFERRABLE) \
         CREATE TABLE LOOSE (K INTEGER, V INTEGER, UNIQUE NULLS NOT DISTINCT (K))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO DEPT VALUES (10, 'Engineering', 'Turin'), (20, 'Operations', 'Milan'), (30, 'Research', 'Rome')").unwrap();
    a.run(
        "INSERT INTO STAFF (ID, NAME, DEPT, SALARY, CODE, NICK) VALUES (1, 'Ada', 10, 7200, 'A001', 'ada'), \
         (2, 'Grace', 10, 8100, 'G002', 'amazing'), (3, 'Edsger', 20, 5400, 'E003', NULL), (4, 'Barbara', NULL, NULL, 'B004', NULL)",
    )
    .unwrap();
    a.run("INSERT INTO LATE VALUES (1, 10), (2, 20)").unwrap();
    a.run("INSERT INTO LOOSE VALUES (1, 10), (NULL, 20)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.7 SR8, GR1e, §4.26.3,
/// Feature T301] a column of a base table's PRIMARY KEY's row is named
/// outside a set function of a query grouped by the key, its value the
/// group's; in HAVING, a window clause, a sort key, a hypothetical set
/// function's value and an inverse distribution function's fraction too; and of a UNIQUE
/// of a column NOT NULL alike, of a generated column by its parametric
/// column, its values those of a grouping by both.
#[test]
fn columns_a_key_determines_are_named_in_its_groups_6_7() {
    let (_db, a) = company();
    let by_key = "SELECT ID, NAME, SALARY, COUNT(*) FROM STAFF GROUP BY ID ORDER BY ID";
    let want = expected(&["1,Ada,7200,1", "2,Grace,8100,1", "3,Edsger,5400,1", "4,Barbara,NULL,1"]);
    assert_eq!(rows(&a, by_key), want);
    assert_eq!(rows(&a, "SELECT ID, NAME, SALARY, COUNT(*) FROM STAFF GROUP BY ID, NAME, SALARY ORDER BY ID"), want);
    assert_eq!(rows(&a, "SELECT ID FROM STAFF GROUP BY ID HAVING SALARY > 6000 ORDER BY ID"), expected(&["1", "2"]));
    let windowed = "SELECT ID, RANK() OVER (PARTITION BY DEPT ORDER BY SALARY DESC) FROM STAFF GROUP BY ID ORDER BY ID";
    assert_eq!(rows(&a, windowed), expected(&["1,2", "2,1", "3,1", "4,1"]));
    let ranked = "SELECT S.ID, RANK(S.SALARY) WITHIN GROUP (ORDER BY T.SALARY) FROM STAFF S, STAFF T GROUP BY S.ID ORDER BY S.ID";
    assert_eq!(rows(&a, ranked), expected(&["1,2", "2,3", "3,1", "4,4"]));
    let fraction = "SELECT S.ID, PERCENTILE_DISC(S.SALARY / 10000.0) WITHIN GROUP (ORDER BY T.SALARY) FROM STAFF S, STAFF T GROUP BY S.ID ORDER BY S.ID";
    assert_eq!(rows(&a, fraction), expected(&["1,8.1E3", "2,8.1E3", "3,7.2E3", "4,NULL"]));
    assert_eq!(
        rows(&a, "SELECT CODE, NAME FROM STAFF GROUP BY CODE ORDER BY CODE"),
        expected(&["A001,Ada", "B004,Barbara", "E003,Edsger", "G002,Grace"])
    );
    assert_eq!(
        rows(&a, "SELECT SALARY, BONUS FROM STAFF GROUP BY SALARY ORDER BY SALARY NULLS LAST"),
        expected(&["5400,540", "7200,720", "8100,810", "NULL,NULL"])
    );
    // A grouping set of each, the key in each.
    let sets = "SELECT ID, NAME, DEPT FROM STAFF GROUP BY GROUPING SETS ((ID), (ID, DEPT)) ORDER BY ID, DEPT";
    assert_eq!(rows(&a, sets).unwrap().len(), 8);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.7 SR8, §4.26.3, §4.26.13]
/// a column no grouping column determines is refused (SQLCODE -103): of a
/// UNIQUE of a nullable column, of a deferrable PRIMARY KEY, of no key, of
/// a grouping set that leaves out the key, and of a key grouped by a
/// COLLATE; a UNIQUE NULLS NOT DISTINCT determines its row (IE015).
#[test]
fn columns_no_grouping_column_determines_are_refused_6_7() {
    let (_db, a) = company();
    for sql in [
        "SELECT NICK, NAME FROM STAFF GROUP BY NICK",
        "SELECT K, V FROM LATE GROUP BY K",
        "SELECT DEPT, NAME FROM STAFF GROUP BY DEPT",
        "SELECT ID, NAME FROM STAFF GROUP BY ROLLUP (ID)",
        "SELECT CODE, NAME FROM STAFF GROUP BY CODE COLLATE CASE_INSENSITIVE",
        "SELECT NAME, COUNT(*) FROM STAFF",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    let e = a.open_cursor("SELECT DEPT, NAME FROM STAFF GROUP BY DEPT").err().unwrap();
    assert!(e.rule.as_deref().is_some_and(|r| r.contains("§6.7 SR8")), "{:?}", e.rule);
    assert!(e.msg.contains("functionally dependent") && e.msg.contains("STAFF.NAME"), "{}", e.msg);
    assert_eq!(rows(&a, "SELECT K, V FROM LOOSE GROUP BY K ORDER BY K NULLS FIRST"), expected(&["NULL,20", "1,10"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.12, §6.7 SR8, GR1e,
/// §7.13 GR2a] of WHERE, a column compared with a literal is determined by
/// none, so named beside a set function of no GROUP BY, null of no rows,
/// and each field of rows compared alike [§4.26.2]; two columns compared
/// determine each other, a column compared by its
/// own collation too, and one compared with an expression of columns is
/// determined by them (IE015).
#[test]
fn equalities_of_where_determine_columns_4_26_12() {
    let (_db, a) = company();
    assert_eq!(rows(&a, "SELECT DEPT, COUNT(*) FROM STAFF WHERE DEPT = 10"), expected(&["10,2"]));
    assert_eq!(rows(&a, "SELECT DEPT, COUNT(*) FROM STAFF WHERE DEPT = 99"), expected(&["NULL,0"]));
    assert!(!a.prepare("SELECT DEPT, COUNT(*) FROM STAFF WHERE DEPT = 10").unwrap().columns()[0].not_null);
    assert!(a.prepare("SELECT ID, NAME FROM STAFF GROUP BY ID").unwrap().columns()[1].not_null);
    assert!(!a.prepare("SELECT NAME, COUNT(*) FROM STAFF WHERE NAME = 'Ada'").unwrap().columns()[0].not_null, "null of no rows");
    let joined = "SELECT S.ID, D.NAME, D.CITY FROM STAFF S, DEPT D WHERE S.DEPT = D.ID GROUP BY S.ID ORDER BY S.ID";
    assert_eq!(rows(&a, joined), expected(&["1,Engineering,Turin", "2,Engineering,Turin", "3,Operations,Milan"]));
    a.run("CREATE TABLE P (X INTEGER, Y INTEGER)").unwrap();
    a.run("INSERT INTO P VALUES (1, 10), (1, 10), (2, 20), (2, 7), (3, 3)").unwrap();
    assert_eq!(rows(&a, "SELECT X, Y, COUNT(*) FROM P WHERE Y = X * 10 GROUP BY X ORDER BY X"), expected(&["1,10,2", "2,20,1"]));
    assert_eq!(rows(&a, "SELECT Y, X FROM P WHERE X = Y GROUP BY Y"), expected(&["3,3"]));
    assert_eq!(rows(&a, "SELECT X, Y, COUNT(*) FROM P WHERE (X, Y) = (2, 20)"), expected(&["2,20,1"]));
    assert_eq!(code(&a, "SELECT X, Y FROM P GROUP BY X"), Code::SyntaxRule);
    // An OR is no AND-component of an equality.
    assert_eq!(code(&a, "SELECT DEPT, COUNT(*) FROM STAFF WHERE DEPT = 10 OR DEPT = 20"), Code::SyntaxRule);
    a.run("CREATE TABLE CI (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE, B INTEGER)").unwrap();
    a.run("INSERT INTO CI VALUES ('Name', 1), ('NAME', 2), ('other', 3)").unwrap();
    assert_eq!(rows(&a, "SELECT UPPER(NAME), COUNT(*) FROM CI WHERE NAME = 'name'"), expected(&["NAME,2"]));
    // By another collation than its own, the column's values may differ.
    a.run("CREATE TABLE BIN (NAME VARCHAR(20), B INTEGER)").unwrap();
    assert_eq!(code(&a, "SELECT NAME, COUNT(*) FROM BIN WHERE NAME COLLATE CASE_INSENSITIVE = 'name'"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.7] of an INNER join, its
/// equalities as WHERE's; of a LEFT join, the first operand's dependencies,
/// and the columns of it the condition references determine the second's
/// column an equality compares, a key of the second still its row's; of a
/// RIGHT join the mirror; of the operand a join extends with nulls, a
/// dependency of a nullable determinant refused; of NATURAL and USING, the
/// common column alike its first operand's.
#[test]
fn joins_keep_and_add_dependencies_4_26_7() {
    let (_db, a) = company();
    let inner = "SELECT S.ID, D.NAME FROM STAFF S JOIN DEPT D ON S.DEPT = D.ID GROUP BY S.ID ORDER BY S.ID";
    assert_eq!(rows(&a, inner), expected(&["1,Engineering", "2,Engineering", "3,Operations"]));
    let left = "SELECT S.ID, S.NAME, D.CITY FROM STAFF S LEFT JOIN DEPT D ON S.DEPT = D.ID GROUP BY S.ID ORDER BY S.ID";
    assert_eq!(rows(&a, left), expected(&["1,Ada,Turin", "2,Grace,Turin", "3,Edsger,Milan", "4,Barbara,NULL"]));
    let right = "SELECT D.ID, D.NAME, COUNT(S.ID) FROM STAFF S RIGHT JOIN DEPT D ON S.DEPT = D.ID GROUP BY D.ID ORDER BY D.ID";
    assert_eq!(rows(&a, right), expected(&["10,Engineering,2", "20,Operations,1", "30,Research,0"]));
    let extended = "SELECT S.ID, S.NAME FROM DEPT D LEFT JOIN STAFF S ON S.DEPT = D.ID GROUP BY S.ID ORDER BY S.ID NULLS FIRST";
    assert_eq!(rows(&a, extended), expected(&["NULL,NULL", "1,Ada", "2,Grace", "3,Edsger"]));
    // A nullable key of the operand extended with nulls: its row and an
    // extended one are alike in it.
    assert_eq!(code(&a, "SELECT L.K, L.V FROM DEPT D LEFT JOIN LOOSE L ON L.V = D.ID * 2 GROUP BY L.K"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT L.K, L.V FROM LOOSE L RIGHT JOIN DEPT D ON L.V = D.ID * 2 GROUP BY L.K"), Code::SyntaxRule);
    assert_eq!(
        rows(&a, "SELECT L.K, L.V FROM LOOSE L LEFT JOIN DEPT D ON L.V = D.ID * 2 GROUP BY L.K ORDER BY 1 NULLS FIRST"),
        expected(&["NULL,20", "1,10"])
    );
    assert_eq!(code(&a, "SELECT D.ID, S.NAME FROM STAFF S RIGHT JOIN DEPT D ON S.DEPT = D.ID GROUP BY D.ID"), Code::SyntaxRule);
    a.run("CREATE TABLE UNIT (DEPT INTEGER PRIMARY KEY, LABEL VARCHAR(10))").unwrap();
    a.run("INSERT INTO UNIT VALUES (10, 'E'), (20, 'O')").unwrap();
    let using = "SELECT DEPT, LABEL, COUNT(*) FROM STAFF JOIN UNIT USING (DEPT) GROUP BY DEPT ORDER BY DEPT";
    assert_eq!(rows(&a, using), expected(&["10,E,2", "20,O,1"]));
    let left_using = "SELECT DEPT, LABEL, COUNT(*) FROM STAFF LEFT JOIN UNIT USING (DEPT) GROUP BY DEPT ORDER BY DEPT NULLS FIRST";
    assert_eq!(rows(&a, left_using), expected(&["NULL,NULL,1", "10,E,2", "20,O,1"]));
    let natural = "SELECT DEPT, LABEL FROM UNIT NATURAL LEFT JOIN STAFF GROUP BY DEPT ORDER BY DEPT";
    assert_eq!(rows(&a, natural), expected(&["10,E", "20,O"]));
    // A COALESCE written of two columns is not alike either, though the
    // first, nullable, determines a column.
    assert_eq!(code(&a, "SELECT C, V FROM (SELECT COALESCE(K, V) AS C, V FROM LOOSE) T GROUP BY C"), Code::SyntaxRule);
    assert_eq!(
        code(&a, "SELECT C, NAME FROM (SELECT COALESCE(S.DEPT, D.ID) AS C, D.NAME FROM STAFF S, DEPT D) T GROUP BY C"),
        Code::SyntaxRule
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.4, §4.26.8, §4.26.15,
/// §4.26.16] of a view, updatable or not, of a derived table, of its
/// computed columns and of a grouped one, of a WITH query, of an EXCEPT,
/// of an INTERSECT, of a table value constructor of one row and of a
/// collection derived table's ordinality column, their queries'
/// dependencies; none of a UNION, of a table value constructor of two rows,
/// or of an ordinality column alone over the rows of a table.
#[test]
fn queries_read_keep_their_dependencies_4_26_16() {
    let (_db, a) = company();
    a.run("CREATE VIEW WHO AS SELECT ID, NAME FROM STAFF").unwrap();
    a.run("CREATE VIEW PLACES AS SELECT S.ID, S.NAME, D.CITY FROM STAFF S JOIN DEPT D ON S.DEPT = D.ID").unwrap();
    assert_eq!(rows(&a, "SELECT ID, NAME FROM WHO GROUP BY ID ORDER BY ID").unwrap().len(), 4);
    assert_eq!(rows(&a, "SELECT ID, CITY FROM PLACES GROUP BY ID ORDER BY ID"), expected(&["1,Turin", "2,Turin", "3,Milan"]));
    let derived = "SELECT ID, TWICE FROM (SELECT ID, SALARY * 2 AS TWICE FROM STAFF) T GROUP BY ID ORDER BY ID";
    assert_eq!(rows(&a, derived), expected(&["1,14400", "2,16200", "3,10800", "4,NULL"]));
    let grouped =
        "SELECT DEPT, TOTAL FROM (SELECT DEPT, SUM(SALARY) AS TOTAL FROM STAFF GROUP BY DEPT) T GROUP BY DEPT ORDER BY DEPT NULLS LAST";
    assert_eq!(rows(&a, grouped), expected(&["10,15300", "20,5400", "NULL,NULL"]));
    let with = "WITH W AS (SELECT ID, NAME FROM STAFF WHERE ID < 3) SELECT ID, NAME FROM W GROUP BY ID ORDER BY ID";
    assert_eq!(rows(&a, with), expected(&["1,Ada", "2,Grace"]));
    let except =
        "SELECT ID, NAME FROM (SELECT ID, NAME FROM STAFF EXCEPT SELECT ID, NAME FROM STAFF WHERE ID = 1) T GROUP BY ID ORDER BY ID";
    assert_eq!(rows(&a, except).unwrap().len(), 3);
    let intersect =
        "SELECT K, L FROM (SELECT S.DEPT, D.NAME FROM STAFF S, DEPT D INTERSECT SELECT ID, NAME FROM DEPT) T (K, L) GROUP BY K ORDER BY K";
    assert_eq!(rows(&a, intersect), expected(&["10,Engineering", "20,Operations"]));
    assert_eq!(rows(&a, "SELECT A, B FROM (VALUES (1, 2)) T (A, B) GROUP BY A"), expected(&["1,2"]));
    let ordinal = "SELECT N, E FROM UNNEST(ARRAY[10, 20]) WITH ORDINALITY AS U (E, N) GROUP BY N ORDER BY N";
    assert_eq!(rows(&a, ordinal), expected(&["1,10", "2,20"]));
    let lateral = "SELECT S.ID, N, E FROM STAFF S, UNNEST(ARRAY[S.ID, S.SALARY]) WITH ORDINALITY AS U (E, N) WHERE S.ID = 1 GROUP BY S.ID, N ORDER BY N";
    assert_eq!(rows(&a, lateral), expected(&["1,1,1", "1,2,7200"]));
    for refused in [
        "SELECT ID, NAME FROM (SELECT ID, NAME FROM STAFF UNION SELECT ID, NAME FROM STAFF) T GROUP BY ID",
        "SELECT A, B FROM (VALUES (1, 2), (1, 3)) T (A, B) GROUP BY A",
        "SELECT N, E FROM STAFF S, UNNEST(ARRAY[S.ID]) WITH ORDINALITY AS U (E, N) GROUP BY N",
    ] {
        assert_eq!(code(&a, refused), Code::SyntaxRule, "{refused}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.7 SR8, §7.14, Feature
/// T301] an outer reference to a grouped query, in a subquery of its HAVING
/// or select list, is to a column functionally dependent on its grouping
/// columns, its value the group's.
#[test]
fn outer_references_to_groups_are_of_dependent_columns_6_7() {
    let (_db, a) = company();
    let having =
        "SELECT ID FROM STAFF S GROUP BY ID HAVING EXISTS (SELECT 1 FROM DEPT D WHERE D.ID = S.DEPT AND D.CITY = 'Turin') ORDER BY ID";
    assert_eq!(rows(&a, having), expected(&["1", "2"]));
    let named = "SELECT ID, (SELECT D.NAME FROM DEPT D WHERE D.ID = S.DEPT) FROM STAFF S GROUP BY ID ORDER BY ID";
    assert_eq!(rows(&a, named), expected(&["1,Engineering", "2,Engineering", "3,Operations", "4,NULL"]));
    assert_eq!(code(&a, "SELECT DEPT FROM STAFF S GROUP BY DEPT HAVING EXISTS (SELECT 1 FROM DEPT D WHERE D.ID = S.ID)"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.7 GR1e; ISO/IEC 9075:1992
/// §6.5 GR1b] the value of a column that is null in its group is kept, no
/// null eliminated, so no warning; a set function's is still warned of.
#[test]
fn a_null_kept_of_a_group_warns_of_nothing_6_7() {
    let (_db, a) = company();
    assert_eq!(rows(&a, "SELECT ID, SALARY FROM STAFF WHERE ID = 4 GROUP BY ID"), expected(&["4,NULL"]));
    assert_eq!(a.warning(), None);
    assert_eq!(rows(&a, "SELECT ID, SUM(SALARY) FROM STAFF WHERE ID = 4 GROUP BY ID"), expected(&["4,NULL"]));
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.9 SR6; ISO/IEC 9075-2:2023
/// §6.7 GR1e] in a constraint, a column functionally dependent on grouping
/// columns of a character string, whose value of its group is chosen
/// (UV070), is possibly non-deterministic, refused; a number is not.
#[test]
fn a_constraint_names_a_dependent_number_alone_6_7() {
    let (_db, a) = company();
    let number = "CREATE ASSERTION PAID CHECK (NOT EXISTS (SELECT ID FROM A.STAFF GROUP BY ID HAVING SALARY < 0))";
    a.run(number).unwrap();
    let text = "CREATE ASSERTION NAMED CHECK (NOT EXISTS (SELECT ID FROM A.STAFF GROUP BY ID HAVING NAME = ''))";
    let e = a.run(text).err().unwrap();
    assert_eq!(e.code, Code::SyntaxRule);
    assert!(e.rule.as_deref().is_some_and(|r| r.contains("§6.7 GR1e")), "{:?}", e.rule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.7 GR1e; RULES.md D38] the
/// values of columns a key determines, of groups past MEMORY.WORK_BYTES,
/// are their rows', row by row and by batches alike.
#[test]
fn dependent_values_of_groups_spilled_are_their_rows_6_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER PRIMARY KEY, V INTEGER, P VARCHAR(40))").unwrap();
    let a = db.session("A");
    let n: u32 = 3000;
    let values: Vec<String> = (1..=n).map(|i| format!("({i}, {}, 'padding padding padding {i}')", i.checked_rem(7).unwrap())).collect();
    a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    let want: Vec<String> = (1..=n).map(|i| format!("{i},{},padding padding padding {i},1", i.checked_rem(7).unwrap())).collect();
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&a, "SELECT I, V, P, COUNT(*) FROM M GROUP BY I ORDER BY I").unwrap(), want, "batches {batches}");
    }
    assert!(spilled.taken() > 0, "the groupings spilled");
}
