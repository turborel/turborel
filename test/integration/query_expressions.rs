//! Query expressions [ISO/IEC 9075:1992 §7.10]: UNION, EXCEPT and
//! INTERSECT, CORRESPONDING, table value constructors [§7.2], TABLE, and
//! select lists with `<as clause>`s and `<qualifier>`.* [§7.9].

use turborel::db::{Completion, Database};
use turborel::error::Code;
use turborel::value::DataType;

use crate::common::{code, exec, expected, rows, setup, sorted};
use turborel::value::Spelled;

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.10 GR1b]: of a row with m
/// duplicates in the first operand and n in the second, UNION gives one if
/// m > 0 or n > 0, UNION ALL m + n; EXCEPT one if m > 0 and n = 0, EXCEPT
/// ALL max(m - n, 0); INTERSECT one if m > 0 and n > 0, INTERSECT ALL
/// min(m, n); nulls are duplicates of each other.
#[test]
fn set_operators_count_duplicates_7_10() {
    let db = setup();
    let s = db.session("APP");
    let cases = [
        ("VALUES 1, 1, 2 UNION VALUES 2, 3", vec!["1", "2", "3"]),
        ("VALUES 1, 1, 2 UNION ALL VALUES 2, 3", vec!["1", "1", "2", "2", "3"]),
        ("VALUES 1, 1, 2, 3 EXCEPT VALUES 1", vec!["2", "3"]),
        ("VALUES 1, 1, 1, 2, 3 EXCEPT ALL VALUES 1, 3, 3", vec!["1", "1", "2"]),
        ("VALUES 1, 1, 1, 2 INTERSECT VALUES 1, 1, 3", vec!["1"]),
        ("VALUES 1, 1, 1, 2 INTERSECT ALL VALUES 1, 1, 3", vec!["1", "1"]),
        ("SELECT NAME FROM T INTERSECT SELECT NAME FROM T WHERE ID = 3", vec!["NULL"]),
        ("SELECT NAME FROM T EXCEPT SELECT NAME FROM T WHERE NAME IS NULL", vec!["ANN   ", "BOB   "]),
    ];
    for (sql, want) in cases {
        assert_eq!(sorted(&s, sql), want, "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.10 F]: INTERSECT binds more
/// tightly than UNION and EXCEPT, which are applied left to right;
/// parentheses group; a set operation is a subquery's query expression
/// [§7.11], also in parentheses of its own.
#[test]
fn set_operations_nest_as_written_7_10() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(sorted(&s, "VALUES 1 UNION VALUES 2 INTERSECT VALUES 3"), vec!["1"]);
    assert_eq!(sorted(&s, "VALUES 1, 2, 3 EXCEPT VALUES 1 EXCEPT VALUES 2"), vec!["3"]);
    assert_eq!(sorted(&s, "VALUES 1, 2, 3 EXCEPT (VALUES 1 EXCEPT VALUES 1)"), vec!["1", "2", "3"]);
    assert_eq!(sorted(&s, "(VALUES 1 UNION VALUES 2) INTERSECT VALUES 2"), vec!["2"]);
    assert_eq!(sorted(&s, "SELECT ID FROM T WHERE ID IN (SELECT ID FROM U UNION SELECT 3 FROM U)"), vec!["1", "3"]);
    assert_eq!(sorted(&s, "SELECT ID FROM T WHERE ID IN ((SELECT ID FROM U) UNION (VALUES 2))"), vec!["1", "2"]);
    assert_eq!(sorted(&s, "SELECT ID FROM T WHERE ID = ((SELECT 2 FROM U WHERE CODE = 'A'))"), vec!["2"]);
    assert_eq!(sorted(&s, "SELECT ID FROM T WHERE ID = ((SELECT 1 FROM U WHERE CODE = 'A') + 1)"), vec!["2"]);
    let derived = "SELECT N FROM (SELECT ID FROM T EXCEPT SELECT ID FROM U) AS X (N)";
    assert_eq!(sorted(&s, derived), vec!["2", "3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.10 SR9b, SR10b, SR12b,
/// §9.3]: the operands are of one degree, each column of the type §9.3
/// gives, named if both operands name it so, and NOT NULL if both are;
/// character strings and numbers do not mix. SQL-86's rules that each
/// operand select only columns, of identical descriptions [X3.135-1986 §8.3
/// SR5b], are Entry SQL's [§7.10, Leveling Rules].
#[test]
fn set_operation_results_take_both_operands_types_7_10() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(sorted(&s, "SELECT ID + 0 FROM T WHERE ID = 1 UNION SELECT AMT FROM T WHERE ID = 2"), vec!["-3.00", "1.00"]);
    assert_eq!(sorted(&s, "SELECT NAME FROM T WHERE ID = 1 UNION SELECT CODE FROM U"), vec!["A     ", "ANN   ", "B     "]);
    assert_eq!(sorted(&s, "SELECT ID FROM U UNION SELECT F FROM T WHERE ID = 1"), vec!["1.5E0", "1E0"]);
    let c = s.open_cursor("SELECT ID, NAME FROM T UNION SELECT ID, CODE FROM U").unwrap();
    assert_eq!(c.columns[0].name.as_deref(), Some("ID"), "SR12b i 1");
    assert_eq!(c.columns[1].name, None, "SR12b i 2");
    assert_eq!(c.columns[1].data_type, DataType::Character { length: 6 });
    assert!(c.columns[0].not_null, "both NOT NULL");
    let c = s.open_cursor("SELECT ID FROM T UNION SELECT AMT AS ID FROM T").unwrap();
    assert!(!c.columns[0].not_null, "AMT may be null");
    assert_eq!(c.columns[0].data_type, DataType::Exact { precision: 12, scale: 2, spelled: Spelled::Numeric });
    assert_eq!(code(&s, "SELECT ID FROM T UNION SELECT CODE FROM U"), Code::SyntaxRule, "§9.3 SR2");
    assert_eq!(code(&s, "SELECT ID FROM T EXCEPT SELECT ID, CODE FROM U"), Code::SyntaxRule, "SR9b");
    assert_eq!(code(&s, "VALUES 1 INTERSECT VALUES 'A'"), Code::SyntaxRule, "SR10b ii");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.1 SR10]: ORDER BY of a set
/// operation names a column of its result by position, or by its name if
/// one column has it, unqualified.
#[test]
fn set_operations_are_ordered_by_result_columns_13_1() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T UNION SELECT ID FROM U ORDER BY ID DESC"), expected(&["3", "2", "1"]));
    assert_eq!(rows(&s, "SELECT ID AS K FROM T EXCEPT SELECT ID FROM U ORDER BY 1 DESC"), expected(&["3", "2"]));
    assert_eq!(code(&s, "SELECT ID FROM T UNION SELECT ID FROM U ORDER BY T.ID"), Code::SyntaxRule, "qualified");
    assert_eq!(code(&s, "SELECT ID FROM T UNION SELECT ID FROM U ORDER BY NAME"), Code::SyntaxRule, "no such column");
    assert_eq!(code(&s, "SELECT ID, ID FROM T UNION SELECT ID, ID FROM U ORDER BY ID"), Code::SyntaxRule, "two of them");
    assert_eq!(code(&s, "SELECT ID FROM T UNION SELECT CODE AS ID FROM U ORDER BY 2"), Code::SyntaxRule, "position");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.10 SR9a]: CORRESPONDING
/// makes each operand the columns both name, in the first's order, or
/// those its BY list names, in that order: names each operand has once, and
/// of both, at least one.
#[test]
fn corresponding_columns_are_matched_by_name_7_10() {
    let db = setup();
    let s = db.session("APP");
    let names = "SELECT ID, NAME FROM T UNION CORRESPONDING SELECT CODE, ID FROM U";
    assert_eq!(sorted(&s, names), vec!["1", "2", "3"]);
    assert_eq!(s.open_cursor(names).unwrap().columns.len(), 1);
    let by = "SELECT ID, CODE AS NAME FROM U EXCEPT CORRESPONDING BY (NAME, ID) SELECT ID, NAME FROM T";
    assert_eq!(sorted(&s, by), vec!["A     ,1", "B     ,1"], "CHARACTER(6)");
    assert_eq!(code(&s, "SELECT ID FROM T UNION CORRESPONDING BY (CODE) SELECT ID, CODE FROM U"), Code::SyntaxRule, "iii 2");
    assert_eq!(code(&s, "SELECT NAME FROM T UNION CORRESPONDING SELECT CODE FROM U"), Code::SyntaxRule, "ii");
    assert_eq!(code(&s, "SELECT ID, ID FROM T UNION CORRESPONDING SELECT ID FROM U"), Code::SyntaxRule, "i");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.2, §7.1]: a table value
/// constructor is the UNION ALL of its rows, each a list in parentheses or
/// one element, all of one degree, its columns of the types §9.3 gives;
/// a row subquery is a row; its expressions may refer outward and be
/// subqueries; NULL and DEFAULT are only an INSERT's [§7.1 SR1].
#[test]
fn table_value_constructors_7_2() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(sorted(&s, "VALUES (1, 'A'), (2.5, 'BB'), (1, 'A')"), vec!["1.0,A ", "1.0,A ", "2.5,BB"]);
    assert_eq!(sorted(&s, "VALUES (SELECT ID, CODE FROM U WHERE CODE = 'B')"), vec!["1,B  "]);
    assert_eq!(sorted(&s, "VALUES (1 + 2) * 3, (4)"), vec!["4", "9"]);
    assert_eq!(sorted(&s, "SELECT * FROM (VALUES (1, 'X'), (2, 'Y')) AS V (N, C) WHERE N > 1"), vec!["2,Y"]);
    assert_eq!(sorted(&s, "SELECT ID FROM T WHERE ID IN (VALUES 1, 3)"), vec!["1", "3"]);
    assert_eq!(sorted(&s, "SELECT (VALUES (SELECT COUNT(*) FROM U)) FROM T WHERE ID = 1"), vec!["2"]);
    let correlated = "SELECT ID FROM T WHERE EXISTS (VALUES T.ID INTERSECT SELECT ID FROM U)";
    assert_eq!(sorted(&s, correlated), vec!["1"]);
    assert_eq!(s.open_cursor("VALUES (1, 2)").unwrap().columns[0].name, None, "§7.10 SR5c");
    assert_eq!(code(&s, "VALUES (1, 2), (3)"), Code::SyntaxRule, "SR1");
    assert_eq!(code(&s, "VALUES (1, NULL)"), Code::SyntaxRule, "§7.1 SR1");
    assert_eq!(code(&s, "VALUES DEFAULT"), Code::SyntaxRule, "§7.1 SR1");
    assert_eq!(code(&s, "VALUES (1), ('A')"), Code::SyntaxRule, "§9.3 SR2");
    assert_eq!(code(&s, "VALUES COUNT(*)"), Code::SyntaxRule, "no argument source");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.10 SR2, SR13b]: TABLE t is
/// (SELECT * FROM t), and a joined table is a query expression, SELECT *
/// FROM it; under EXISTS, * of a set operation's operand is its columns,
/// not the literal it is for a query specification alone [§7.9 SR3a].
#[test]
fn explicit_and_joined_tables_are_queries_7_10() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(sorted(&s, "TABLE U"), vec!["1,A  ", "1,B  "]);
    assert_eq!(sorted(&s, "TABLE U EXCEPT SELECT ID, CODE FROM U WHERE CODE = 'A'"), vec!["1,B  "]);
    assert_eq!(sorted(&s, "SELECT ID FROM T WHERE EXISTS (TABLE U EXCEPT TABLE U)"), Vec::<String>::new());
    assert_eq!(sorted(&s, "T NATURAL JOIN U"), vec!["1,ANN   ,10.50,1.5E0,A  ", "1,ANN   ,10.50,1.5E0,B  "]);
    assert_eq!(sorted(&s, "(T CROSS JOIN U) CROSS JOIN U AS V").len(), 12);
    assert_eq!(sorted(&s, "((T NATURAL JOIN U) CROSS JOIN U AS V)").len(), 4);
    let exists = "SELECT ID FROM T WHERE EXISTS (SELECT * FROM U WHERE U.ID = T.ID EXCEPT SELECT * FROM U WHERE CODE = 'A')";
    assert_eq!(sorted(&s, exists), vec!["1"]);
    assert_eq!(code(&s, "T"), Code::Syntax, "a table alone is no <joined table>");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.9 SR4, SR9a, §6.4 SR2b]: an
/// `<as clause>`, with AS or not, names its column, which ORDER BY may
/// name; `<qualifier>`.* is the columns of the table it designates, but
/// for a NATURAL or USING join's common columns, and names one in scope.
#[test]
fn select_lists_name_and_qualify_columns_7_9() {
    let db = setup();
    let s = db.session("APP");
    let c = s.open_cursor("SELECT ID AS N, NAME X, AMT FROM T ORDER BY N DESC").unwrap();
    let names: Vec<Option<&str>> = c.columns.iter().map(|c| c.name.as_deref()).collect();
    assert_eq!(names, vec![Some("N"), Some("X"), Some("AMT")]);
    assert_eq!(rows(&s, "SELECT ID AS N FROM T ORDER BY N DESC"), expected(&["3", "2", "1"]));
    assert_eq!(rows(&s, "SELECT ID + 1 AS N FROM T ORDER BY N"), expected(&["2", "3", "4"]));
    assert_eq!(sorted(&s, "SELECT U.*, T.NAME FROM T, U WHERE T.ID = U.ID"), vec!["1,A  ,ANN   ", "1,B  ,ANN   "]);
    assert_eq!(sorted(&s, "SELECT X.* FROM T AS X WHERE X.ID = 2"), vec!["2,BOB   ,-3.00,2E0"]);
    assert_eq!(sorted(&s, "SELECT APP.U.* FROM U WHERE CODE = 'A'"), vec!["1,A  "]);
    assert_eq!(sorted(&s, "SELECT ID, T.*, U.* FROM T NATURAL JOIN U"), vec!["1,ANN   ,10.50,1.5E0,A  ", "1,ANN   ,10.50,1.5E0,B  "]);
    assert_eq!(code(&s, "SELECT V.* FROM T"), Code::SyntaxRule, "no V in scope");
    assert_eq!(code(&s, "SELECT T.* FROM T AS X"), Code::SyntaxRule, "X hides T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.8, §7.1 GR2]: INSERT takes
/// any query expression: a table value constructor of many rows, whose
/// NULL is null and DEFAULT its column's default, also under a set
/// operation, but not in a subquery; each value fits its column
/// [X3.135-1986 §8.7 SR6]; the degree is the column list's [SR5].
#[test]
fn insert_takes_query_expressions_13_8() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE R (K INTEGER NOT NULL, C CHAR(2) DEFAULT 'D', N DECIMAL(4,1))").unwrap();
    let s = db.session("Q");
    assert_eq!(exec(&s, "INSERT INTO R VALUES (1, 'A', 1.5), (2, DEFAULT, NULL), (3, NULL, 7)"), Ok(Completion::Success));
    assert_eq!(exec(&s, "INSERT INTO R (K, N) VALUES (4, DEFAULT) UNION ALL VALUES (5, 2)"), Ok(Completion::Success));
    assert_eq!(exec(&s, "INSERT INTO R (K) SELECT K + 10 FROM R WHERE K < 3 EXCEPT VALUES 12"), Ok(Completion::Success));
    assert_eq!(exec(&s, "INSERT INTO R (K) TABLE R"), Err(Code::SyntaxRule), "SR5: degree 3 for one column");
    assert_eq!(sorted(&s, "SELECT * FROM R"), vec!["1,A ,1.5", "11,D ,NULL", "2,D ,NULL", "3,NULL,7.0", "4,D ,NULL", "5,D ,2.0"]);
    assert_eq!(exec(&s, "INSERT INTO R VALUES (6, 'TOO', 1)"), Err(Code::Data), "§9.2 GR3b ii");
    assert_eq!(exec(&s, "INSERT INTO R VALUES (6, 1, 1)"), Err(Code::SyntaxRule), "§9.2 SR2");
    assert_eq!(exec(&s, "INSERT INTO R VALUES (6, 'A')"), Err(Code::SyntaxRule), "SR5");
    assert_eq!(exec(&s, "INSERT INTO R (K) VALUES ((VALUES NULL))"), Err(Code::SyntaxRule), "§7.1 SR1");
    assert_eq!(exec(&s, "INSERT INTO R (K) VALUES (NULL)"), Err(Code::Constraint), "K is NOT NULL");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.19 SR6, SR7, SR8, §7.10
/// SR4]: a view's query is a query expression: one with a set operation is
/// read-only and not grouped, and one whose columns are unnamed needs a
/// column list; one of `<qualifier>`.* may be updatable; a column added to
/// a table a view reads by TABLE or `<qualifier>`.* leaves the view as it
/// was [§11.11 GR2, note].
#[test]
fn views_of_query_expressions_11_19() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE A (K INTEGER)
           CREATE TABLE B (K INTEGER)
           CREATE VIEW AB AS SELECT K FROM A UNION SELECT K FROM B
           CREATE VIEW TA AS TABLE A
           CREATE VIEW PA AS (SELECT * FROM A)
           CREATE VIEW V (N) AS VALUES 1, 2
           CREATE VIEW BS AS SELECT B.* FROM B
           CREATE VIEW NB (N) AS VALUES ((SELECT COUNT(*) FROM B))",
    )
    .unwrap();
    let s = db.session("Q");
    for sql in ["INSERT INTO A VALUES 1, 2", "INSERT INTO B VALUES 2, 3", "INSERT INTO TA VALUES 4"] {
        assert_eq!(exec(&s, sql), Ok(Completion::Success), "{sql}");
    }
    assert_eq!(sorted(&s, "SELECT K FROM AB"), vec!["1", "2", "3", "4"]);
    assert_eq!(sorted(&s, "SELECT N FROM V WHERE N > 1"), vec!["2"]);
    assert_eq!(exec(&s, "DELETE FROM AB"), Err(Code::SyntaxRule), "read-only");
    assert_eq!(db.schema("CREATE SCHEMA AUTHORIZATION Z CREATE VIEW W AS VALUES 1").err().map(|e| e.code), Some(Code::SyntaxRule), "SR8");
    assert_eq!(exec(&s, "INSERT INTO BS VALUES 5"), Ok(Completion::Success), "B.* is updatable");
    assert_eq!(sorted(&s, "SELECT N FROM NB"), vec!["3"]);
    assert_eq!(exec(&s, "ALTER TABLE A ADD COLUMN C INTEGER"), Ok(Completion::Success));
    assert_eq!(sorted(&s, "SELECT * FROM TA"), vec!["1", "2", "4"]);
    assert_eq!(sorted(&s, "SELECT * FROM PA"), vec!["1", "2", "4"]);
    assert_eq!(exec(&s, "ALTER TABLE B ADD COLUMN C INTEGER"), Ok(Completion::Success));
    assert_eq!(sorted(&s, "SELECT * FROM BS"), vec!["2", "3", "5"], "BS's B.* is B's K alone");
    assert_eq!(exec(&s, "INSERT INTO BS VALUES 6"), Ok(Completion::Success), "and still updatable");
}
