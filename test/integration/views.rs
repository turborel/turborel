//! Viewed tables [X3.135-1986 §4.4, §6.5]: reading them, the rules of
//! their definition, grouped views [§5.19 SR1, §5.20 SR5, §5.24 SR6, SR9,
//! §5.25 SR6, §8.10 SR2], changing a base table through an updatable view
//! [§6.5 GR2] WITH CHECK OPTION or not [GR3], cursors over them, and the
//! privileges of their owners [§6.6 SR3b i 2, 3].

use turborel::db::{Completion, Database};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Value};

use crate::common::{code, exec, expected, rows};
use turborel::value::Spelled;

/// Schema S: tables E (employees) and D (departments), and views over
/// them: RICH (E's rows with PAY over 100), RICH_C (the same, renamed, WITH
/// CHECK OPTION), RICH_A (RICH's with ID under 10), STAFF (a join), DEPTS
/// (grouped), NAME_LIST (DISTINCT). Q holds SELECT on E; Q2 SELECT and UPDATE
/// of PAY, grantable; R SELECT on RICH and all privileges on RICH_C.
fn views() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE E (ID INTEGER NOT NULL UNIQUE, NAME CHAR(6), DEPT CHAR(3), PAY DECIMAL(7,2))
           CREATE TABLE D (DEPT CHAR(3) NOT NULL UNIQUE, CITY CHAR(6))
           CREATE VIEW RICH AS SELECT ID, NAME, PAY FROM E WHERE PAY > 100
           CREATE VIEW RICH_C (K, N, P) AS SELECT ID, NAME, PAY FROM E WHERE PAY > 100 WITH CHECK OPTION
           CREATE VIEW RICH_A AS SELECT * FROM RICH WHERE ID < 10
           CREATE VIEW STAFF AS SELECT NAME, CITY FROM E, D WHERE E.DEPT = D.DEPT
           CREATE VIEW DEPTS (DEPT, N, TOTAL) AS SELECT DEPT, COUNT(*), SUM(PAY) FROM E GROUP BY DEPT
           CREATE VIEW NAME_LIST AS SELECT DISTINCT NAME FROM E
           GRANT SELECT ON E TO Q
           GRANT SELECT, UPDATE (PAY) ON E TO Q2 WITH GRANT OPTION
           GRANT SELECT ON RICH TO R
           GRANT ALL PRIVILEGES ON RICH_C TO R",
    )
    .unwrap();
    let s = db.session("S");
    for row in ["1, 'ANN', 'A', 50", "2, 'BOB', 'A', 150", "3, 'CY', 'B', 300", "12, 'DI', 'B', 200"] {
        s.run(&format!("INSERT INTO E VALUES ({row})")).unwrap();
    }
    s.run("INSERT INTO D VALUES ('A', 'OSLO')").unwrap();
    s.run("INSERT INTO D VALUES ('B', 'ROME')").unwrap();
    drop(s);
    db
}

/// Verifies: [X3.135-1986 §6.5 GR1, SR7] a view is the table its query
/// gives, its columns named as its `<view column list>` says or as the
/// query's; views over views, joins, grouped and DISTINCT queries; a view
/// is a table in a FROM clause, with a correlation name, in a subquery; an
/// updatable view's rows are found through the base table's index.
#[test]
fn views_are_the_tables_their_queries_give_6_5() {
    let db = views();
    let s = db.session("S");
    assert_eq!(rows(&s, "SELECT * FROM RICH ORDER BY ID"), expected(&["2,BOB   ,150.00", "3,CY    ,300.00", "12,DI    ,200.00"]));
    assert_eq!(rows(&s, "SELECT K, P FROM RICH_C WHERE N = 'CY'"), expected(&["3,300.00"]));
    assert_eq!(code(&s, "SELECT ID FROM RICH_C"), Code::SyntaxRule, "renamed K");
    assert_eq!(rows(&s, "SELECT ID FROM RICH_A ORDER BY ID"), expected(&["2", "3"]));
    assert_eq!(
        rows(&s, "SELECT NAME, CITY FROM STAFF ORDER BY NAME"),
        expected(&["ANN   ,OSLO  ", "BOB   ,OSLO  ", "CY    ,ROME  ", "DI    ,ROME  "])
    );
    assert_eq!(rows(&s, "SELECT * FROM DEPTS ORDER BY DEPT"), expected(&["A  ,2,200.00", "B  ,2,500.00"]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM NAME_LIST"), expected(&["4"]));
    assert_eq!(
        rows(&s, "SELECT X.NAME, D.CITY FROM RICH X, E, D WHERE X.ID = E.ID AND E.DEPT = D.DEPT AND X.ID = 2"),
        expected(&["BOB   ,OSLO  "])
    );
    assert_eq!(rows(&s, "SELECT ID FROM E WHERE ID IN (SELECT ID FROM RICH_A) ORDER BY ID"), expected(&["2", "3"]));
    assert_eq!(
        rows(&s, "SELECT NAME FROM E WHERE EXISTS (SELECT * FROM NAME_LIST WHERE NAME_LIST.NAME = E.NAME AND E.ID = 1)"),
        expected(&["ANN   "])
    );
    assert_eq!(rows(&s, "SELECT NAME FROM RICH WHERE ID = 1"), expected(&[]), "ANN's PAY is 50");
    for id in 100..400 {
        s.run(&format!("INSERT INTO E VALUES ({id}, 'N', 'A', {id})")).unwrap();
    }
    let plan = s.physical_plan("SELECT NAME FROM RICH WHERE ID = 300", &Params::new()).unwrap();
    assert!(plan.contains("IndexScan"), "{plan}");
    assert_eq!(rows(&s, "SELECT NAME FROM RICH WHERE ID = 300"), expected(&["N     "]));
}

/// Verifies: [X3.135-1986 §6.5 SR1, SR2, SR4-SR6, SR9] a view is in its
/// schema and named apart from its tables and views; a `<view column
/// list>` where the query has unnamed or like-named columns, naming each
/// column once and as many as the query has; WITH CHECK OPTION only on an
/// updatable view; its query names defined tables, and earlier views, the
/// schema can read [§5.4 SR5, §5.25 SR1, §6.6 SR3a i].
#[test]
fn view_definitions_6_5() {
    let db = views();
    let fails = |sql: &str| db.schema(sql).map(drop).map_err(|e| e.code);
    let table = "CREATE SCHEMA AUTHORIZATION V CREATE TABLE T (A INT, B INT) ";
    assert_eq!(fails(&format!("{table} CREATE VIEW S.W AS SELECT A FROM T")), Err(Code::SyntaxRule), "SR1");
    assert_eq!(fails(&format!("{table} CREATE VIEW T AS SELECT A FROM T")), Err(Code::SyntaxRule), "SR2");
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT A FROM T CREATE VIEW W AS SELECT B FROM T")), Err(Code::SyntaxRule), "SR2");
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT A + 1 FROM T")), Err(Code::SyntaxRule), "SR4: unnamed");
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT T.A, X.A FROM T, T X")), Err(Code::SyntaxRule), "SR4: A twice");
    assert_eq!(fails(&format!("{table} CREATE VIEW W (C, C) AS SELECT A, B FROM T")), Err(Code::SyntaxRule), "SR5");
    assert_eq!(fails(&format!("{table} CREATE VIEW W (C) AS SELECT A, B FROM T")), Err(Code::SyntaxRule), "SR6");
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT DISTINCT A FROM T WITH CHECK OPTION")), Err(Code::SyntaxRule), "SR9");
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT A FROM NOPE")), Err(Code::SyntaxRule), "§5.4 SR5");
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT A FROM X CREATE VIEW X AS SELECT A FROM T")), Err(Code::SyntaxRule));
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT NAME FROM S.RICH")), Err(Code::Privilege), "§5.25 SR1");
    assert_eq!(fails(&format!("{table} CREATE VIEW W AS SELECT A FROM T WITH OPTION")), Err(Code::Syntax));
    assert_eq!(fails(&format!("{table} CREATE VIEW W SELECT A FROM T")), Err(Code::Syntax));
    db.schema(&format!("{table} CREATE VIEW W (C, D) AS SELECT A + 1, A FROM T CREATE VIEW X AS SELECT C FROM W WHERE D > 0")).unwrap();
    let v = db.session("V");
    v.run("INSERT INTO T VALUES (1, 2)").unwrap();
    assert_eq!(rows(&v, "SELECT C FROM X"), expected(&["2"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.4, §7.9, §7.11, §13.5,
/// Leveling Rules] a grouped view [X3.135-1986 §6.5 SR8] is read as any
/// table: with WHERE, GROUP BY or HAVING over it, a set function on its
/// columns, beside another table, in a comparison's subquery or a
/// single-row SELECT, all of which SQL-86 refuses [X3.135-1986 §5.19 SR1,
/// §5.20 SR5, §5.24 SR6, SR9, §5.25 SR6, §8.10 SR2] and SQL-92 only for
/// Entry SQL.
#[test]
fn grouped_views_are_read_as_tables_7_4() {
    let db = views();
    let s = db.session("S");
    assert_eq!(rows(&s, "SELECT DEPT FROM DEPTS WHERE TOTAL > 300"), expected(&["B  "]));
    assert_eq!(rows(&s, "SELECT DEPT, SUM(N) FROM DEPTS GROUP BY DEPT ORDER BY DEPT"), expected(&["A  ,2", "B  ,2"]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM DEPTS, D"), expected(&["4"]));
    assert_eq!(rows(&s, "SELECT SUM(TOTAL) FROM DEPTS"), expected(&["700.00"]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM E WHERE EXISTS (SELECT MAX(N) FROM DEPTS)"), expected(&["4"]));
    assert_eq!(code(&s, "SELECT ID FROM E WHERE PAY = (SELECT TOTAL FROM DEPTS)"), Code::Cardinality, "two rows");
    assert_eq!(rows(&s, "SELECT ID FROM E WHERE PAY * 2 IN (SELECT TOTAL FROM DEPTS) ORDER BY ID"), expected(&[]));
    let mut p = Params::new().declare("N", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    assert_eq!(s.execute("SELECT TOTAL INTO N FROM DEPTS", &mut p).unwrap_err().code, Code::Cardinality, "two rows");
    s.execute("SELECT MAX(TOTAL) INTO N FROM DEPTS", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "500");
}

/// Verifies: [X3.135-1986 §6.5 GR2, GR3b, §5.25 SR11] INSERT, UPDATE and
/// DELETE through an updatable view change the base table's rows: those of
/// the view, the inserted row null in the base table's other columns; with
/// no CHECK OPTION a row need not be in the view; a read-only view, as is
/// one over a read-only view, takes no change [§4.4, §5.25 SR11c, §8.4
/// SR3, §8.5 SR2, §8.7 SR2, §8.12 SR2].
#[test]
fn changes_through_updatable_views_6_5() {
    let db = views();
    let s = db.session("S");
    assert_eq!(exec(&s, "INSERT INTO RICH VALUES (20, 'EVE', 500)"), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT ID, NAME, DEPT, PAY FROM E WHERE ID = 20"), expected(&["20,EVE   ,NULL,500.00"]));
    assert_eq!(exec(&s, "INSERT INTO RICH (ID, PAY) VALUES (21, 5)"), Ok(Completion::Success), "GR3b");
    assert_eq!(rows(&s, "SELECT ID FROM RICH WHERE ID > 19 ORDER BY ID"), expected(&["20"]));
    assert_eq!(exec(&s, "UPDATE RICH SET PAY = PAY + 1 WHERE NAME = 'ANN'"), Ok(Completion::NoData), "ANN is not in it");
    assert_eq!(exec(&s, "UPDATE RICH_A SET NAME = 'BO'"), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT ID FROM E WHERE NAME = 'BO' ORDER BY ID"), expected(&["2", "3"]));
    assert_eq!(exec(&s, "DELETE FROM RICH_A WHERE ID > 2"), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT ID FROM E ORDER BY ID"), expected(&["1", "2", "12", "20", "21"]));
    assert_eq!(exec(&s, "INSERT INTO RICH (NAME) VALUES ('Z')"), Err(Code::Constraint), "E.ID is NOT NULL");
    assert_eq!(exec(&s, "INSERT INTO RICH VALUES (2, 'Z', 900)"), Err(Code::Constraint), "E.ID is UNIQUE");
    assert_eq!(exec(&s, "INSERT INTO RICH SELECT ID + 100, NAME, PAY FROM E WHERE ID = 1"), Ok(Completion::Success));
    assert_eq!(exec(&s, "INSERT INTO RICH SELECT * FROM RICH"), Err(Code::Constraint), "E.ID is UNIQUE: §8.7 SR2 is lifted");
    assert_eq!(exec(&s, "INSERT INTO STAFF VALUES ('X', 'Y')"), Err(Code::SyntaxRule), "a join");
    assert_eq!(exec(&s, "UPDATE DEPTS SET N = 0"), Err(Code::SyntaxRule), "grouped");
    assert_eq!(exec(&s, "DELETE FROM NAME_LIST"), Err(Code::SyntaxRule), "DISTINCT");
    assert_eq!(exec(&s, "UPDATE RICH SET DEPT = 'A'"), Err(Code::SyntaxRule), "not a column of RICH");
    // [§5.25 SR11c] a query of an updatable form over a read-only table.
    db.schema("CREATE SCHEMA AUTHORIZATION S3 CREATE TABLE T (A INT) CREATE VIEW DA AS SELECT DISTINCT A FROM T CREATE VIEW OVER AS SELECT A FROM DA")
        .unwrap();
    let s3 = db.session("S3");
    assert_eq!(exec(&s3, "INSERT INTO OVER VALUES (1)"), Err(Code::SyntaxRule));
    s3.run("INSERT INTO T VALUES (1)").unwrap();
    let mut p = Params::new().declare("OUT", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    s3.declare_cursor("DECLARE C CURSOR FOR SELECT A FROM OVER").unwrap();
    s3.execute("OPEN C", &mut p).unwrap();
    s3.execute("FETCH C INTO OUT", &mut p).unwrap();
    assert_eq!(s3.execute("DELETE FROM OVER WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::SyntaxRule, "a read-only cursor");
}

/// Verifies: [X3.135-1986 §6.5 GR3a, §8.7 GR2, §8.11 GR5, §8.12 GR4] a row
/// inserted or updated through a view WITH CHECK OPTION makes its WHERE
/// condition true, not false or unknown, or the statement fails and changes
/// nothing; [ISO/IEC 9075:1992 §11.19 GR11] so does one through a view over
/// it without the option, which spans it.
#[test]
fn with_check_option_6_5() {
    let db = views();
    let s = db.session("S");
    assert_eq!(exec(&s, "INSERT INTO RICH_C VALUES (30, 'X', 5)"), Err(Code::Constraint));
    assert_eq!(exec(&s, "INSERT INTO RICH_C (K, N) VALUES (31, 'Y')"), Err(Code::Constraint), "unknown");
    assert_eq!(exec(&s, "INSERT INTO RICH_C VALUES (30, 'X', 500)"), Ok(Completion::Success));
    assert_eq!(exec(&s, "UPDATE RICH_C SET P = 1 WHERE K = 2"), Err(Code::Constraint));
    assert_eq!(exec(&s, "UPDATE RICH_C SET P = P - 100"), Err(Code::Constraint), "BOB's would be 50");
    assert_eq!(rows(&s, "SELECT PAY FROM E WHERE ID = 2"), expected(&["150.00"]), "none changed");
    assert_eq!(exec(&s, "UPDATE RICH_C SET P = P + 1"), Ok(Completion::Success));
    let mut p = Params::new().declare("OUT", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM RICH_C WHERE K = 3").unwrap();
    s.execute("OPEN C", &mut p).unwrap();
    s.execute("FETCH C INTO OUT", &mut p).unwrap();
    assert_eq!(s.execute("UPDATE RICH_C SET P = 0 WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::Constraint, "§8.11 GR5");
    s.execute("UPDATE RICH_C SET P = 400 WHERE CURRENT OF C", &mut p).unwrap();
    drop(s);
    db.schema("CREATE SCHEMA AUTHORIZATION S2 CREATE TABLE T (A INT) CREATE VIEW C AS SELECT A FROM T WHERE A > 0 WITH CHECK OPTION CREATE VIEW LOOSE AS SELECT A FROM C").unwrap();
    let s2 = db.session("S2");
    assert_eq!(exec(&s2, "INSERT INTO C VALUES (-1)"), Err(Code::Constraint));
    assert_eq!(exec(&s2, "INSERT INTO LOOSE VALUES (-1)"), Err(Code::Constraint), "LOOSE spans C");
    assert_eq!(exec(&s2, "INSERT INTO LOOSE VALUES (1)"), Ok(Completion::Success));
    assert_eq!(rows(&s2, "SELECT A FROM T"), expected(&["1"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.19 SR13, GR9, GR11, GR12]:
/// WITH CHECK OPTION is CASCADED unless LOCAL is written. An update
/// operation is checked against every view WITH CHECK OPTION it spans: a
/// CASCADED one refuses a row that does not appear in it, under the
/// conditions of the views under it too; a LOCAL one only a row that
/// appears in the table its query reads but not in it. The Information
/// Schema shows each.
#[test]
fn check_option_levels_11_19() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION L
           CREATE TABLE T (A INT)
           CREATE VIEW POSITIVE AS SELECT A FROM T WHERE A > 0
           CREATE VIEW LOCAL_SMALL AS SELECT A FROM POSITIVE WHERE A < 10 WITH LOCAL CHECK OPTION
           CREATE VIEW SMALL AS SELECT A FROM POSITIVE WHERE A < 10 WITH CASCADED CHECK OPTION
           CREATE VIEW PLAIN AS SELECT A FROM POSITIVE WHERE A < 10 WITH CHECK OPTION
           CREATE VIEW OVER_LOCAL AS SELECT A FROM LOCAL_SMALL WHERE A <> 5
           CREATE VIEW ALL_OF_LOCAL AS SELECT A FROM LOCAL_SMALL WITH CHECK OPTION",
    )
    .unwrap();
    let s = db.session("L");
    let refused = Err(Code::Constraint);
    let done = Ok(Completion::Success);
    for (sql, outcome) in [
        ("INSERT INTO LOCAL_SMALL VALUES (20)", &refused),
        ("INSERT INTO LOCAL_SMALL VALUES (-1)", &done),
        ("INSERT INTO SMALL VALUES (20)", &refused),
        ("INSERT INTO SMALL VALUES (-2)", &refused),
        ("INSERT INTO PLAIN VALUES (-2)", &refused),
        ("INSERT INTO OVER_LOCAL VALUES (20)", &refused),
        ("INSERT INTO OVER_LOCAL VALUES (5)", &done),
        ("INSERT INTO OVER_LOCAL VALUES (-3)", &done),
        ("INSERT INTO ALL_OF_LOCAL VALUES (-4)", &refused),
        ("INSERT INTO ALL_OF_LOCAL VALUES (20)", &refused),
        ("INSERT INTO SMALL VALUES (3)", &done),
        ("UPDATE LOCAL_SMALL SET A = 50", &refused),
        ("UPDATE LOCAL_SMALL SET A = -A", &done),
    ] {
        assert_eq!(&exec(&s, sql), outcome, "{sql}");
    }
    assert_eq!(rows(&s, "SELECT A FROM T ORDER BY A"), expected(&["-5", "-3", "-3", "-1"]));
    let shown = "SELECT TRIM(TABLE_NAME), TRIM(CHECK_OPTION) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = 'L' ORDER BY 1";
    assert_eq!(
        rows(&s, shown),
        expected(&["ALL_OF_LOCAL,CASCADED", "LOCAL_SMALL,LOCAL", "OVER_LOCAL,NONE", "PLAIN,CASCADED", "POSITIVE,NONE", "SMALL,CASCADED"])
    );
}

/// Verifies: [X3.135-1986 §5.25 SR11c, §8.3 SR4, §8.4 SR3, SR4, §8.11 SR4]
/// a cursor over an updatable view is updatable, its positioned UPDATE and
/// DELETE naming the view and changing the base table's row; one over a
/// read-only view is read-only.
#[test]
fn cursors_over_views_8_3() {
    let db = views();
    let s = db.session("S");
    let mut p = Params::new().declare("K", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM RICH_A").unwrap();
    s.execute("OPEN C", &mut p).unwrap();
    s.execute("FETCH C INTO K", &mut p).unwrap();
    let first = p.value("K").to_string();
    assert_eq!(s.execute("DELETE FROM E WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::SyntaxRule, "§8.4 SR4");
    s.execute("UPDATE RICH_A SET NAME = 'NEW' WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(rows(&s, &format!("SELECT NAME FROM E WHERE ID = {first}")), expected(&["NEW   "]));
    s.execute("DELETE FROM RICH_A WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(rows(&s, &format!("SELECT NAME FROM E WHERE ID = {first}")), expected(&[]));
    s.declare_cursor("DECLARE R CURSOR FOR SELECT NAME FROM STAFF").unwrap();
    s.execute("OPEN R", &mut p).unwrap();
    let mut q = Params::new().declare("N", DataType::Character { length: 6 }, Value::Null);
    s.execute("FETCH R INTO N", &mut q).unwrap();
    assert_eq!(s.execute("DELETE FROM STAFF WHERE CURRENT OF R", &mut q).unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §6.6 SR3b i 2, 3, SR3b ii] the owner of a
/// read-only view holds SELECT on it, grantable if its SELECT on every
/// table the view names is; of an updatable view, what it holds on the
/// view's table, column for column; others what is granted on the view,
/// and nothing on its tables through it. [ISO/IEC 9075:1992 §11.36 GR14]
/// What its owner cannot grant is not granted, with a warning.
#[test]
fn privileges_on_views_6_6() {
    let db = views();
    let r = db.session("R");
    assert_eq!(rows(&r, "SELECT ID FROM S.RICH ORDER BY ID"), expected(&["2", "3", "12"]));
    assert_eq!(code(&r, "SELECT ID FROM S.E"), Code::Privilege);
    assert_eq!(exec(&r, "INSERT INTO S.RICH VALUES (40, 'R', 400)"), Err(Code::Privilege));
    assert_eq!(exec(&r, "INSERT INTO S.RICH_C VALUES (40, 'R', 400)"), Ok(Completion::Success));
    let warned = Ok(Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED));
    assert_eq!(
        db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE VIEW QV AS SELECT DISTINCT NAME FROM S.E GRANT SELECT ON QV TO P")
            .map_err(|e| e.code),
        warned,
        "SR3b i 2"
    );
    assert_eq!(code(&db.session("P"), "SELECT NAME FROM Q.QV"), Code::Privilege, "not granted");
    let q = db.session("Q");
    assert_eq!(exec(&q, "CREATE VIEW QU AS SELECT ID, NAME FROM S.E"), Ok(Completion::Success));
    assert_eq!(exec(&q, "GRANT INSERT ON QU TO P"), warned, "SR3b i 3: SELECT only, not grantable");
    q.commit().unwrap();
    assert_eq!(exec(&db.session("Q"), "INSERT INTO QU VALUES (50, 'Q')"), Err(Code::Privilege), "Q holds SELECT on E only");
    assert_eq!(rows(&db.session("Q"), "SELECT ID FROM QU WHERE ID = 1"), expected(&["1"]));
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q2
           CREATE VIEW Q2V AS SELECT ID, PAY FROM S.E
           CREATE VIEW Q2N AS SELECT DISTINCT PAY FROM S.E
           GRANT UPDATE (PAY) ON Q2V TO P2
           GRANT SELECT ON Q2N TO P2",
    )
    .unwrap();
    let p2 = db.session("P2");
    assert_eq!(exec(&p2, "UPDATE Q2.Q2V SET PAY = 1 WHERE ID = 1"), Ok(Completion::Success));
    assert_eq!(exec(&p2, "UPDATE Q2.Q2V SET ID = 1"), Err(Code::Privilege));
    assert_eq!(rows(&p2, "SELECT COUNT(*) FROM Q2.Q2N"), expected(&["4"]));
}

/// Verifies: [DESIGN] RULES.md D35, `PUSH_FILTER_THROUGH_PROJECT`,
/// `PUSH_FILTER_THROUGH_DISTINCT`: the WHERE clause of a query on a view
/// that is not updatable is pushed into the view's plan: below a joined
/// view's projection, then into its product, to D's rows; below DISTINCT.
/// A LIKE is not pushed below DISTINCT: it can tell 'BOB' from 'BOB '. (A
/// grouped view has no WHERE clause over it [X3.135-1986 §5.19 SR1].) The
/// rows are those without the rules [X3.135-1986 §6.5 GR1].
#[test]
fn filters_reach_into_views_6_5() {
    let db = views();
    let s = db.session("S");
    let queries = [
        ("SELECT NAME FROM STAFF WHERE CITY = 'ROME'", vec!["push_filter_through_project", "push_filter_into_product"]),
        ("SELECT NAME FROM NAME_LIST WHERE NAME > 'B'", vec!["push_filter_through_distinct", "push_filter_through_project"]),
        ("SELECT NAME FROM NAME_LIST WHERE NAME LIKE 'B%'", vec![]),
    ];
    let rules = ["push_filter_through_project", "push_filter_through_distinct", "push_filter_into_product"];
    let mut found = Vec::new();
    for (sql, fired) in &queries {
        let plan = s.explain(sql, &Params::new()).unwrap();
        let trace = plan.lines().find(|l| l.contains("-- rules:")).unwrap_or_default().to_string();
        for rule in rules {
            assert_eq!(trace.contains(rule), fired.contains(&rule), "{sql}: {rule} in {trace}");
        }
        found.push(crate::common::sorted(&s, sql));
    }
    for rule in rules {
        assert!(db.with_optimizer(|o| o.set_enabled(rule, false)).unwrap());
    }
    let s = db.session("S");
    let without: Vec<_> = queries.iter().map(|(sql, _)| crate::common::sorted(&s, sql)).collect();
    assert_eq!(found, without);
    assert!(found.iter().all(|rows| !rows.is_empty()), "{found:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.26, §6.2 GR9]: CURRENT_USER
/// in a view is the current authorization identifier of the statement
/// reading it, not the view's owner, whose privileges the view's query is
/// bound with; so a view of the rows of whoever reads it shows each user
/// theirs, through an updatable view too.
#[test]
fn current_user_in_a_view_is_its_reader_4_26() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O
           CREATE TABLE T (WHO CHAR(8), N INTEGER)
           CREATE VIEW MINE AS SELECT * FROM T WHERE WHO = CURRENT_USER WITH CHECK OPTION
           GRANT SELECT, INSERT ON MINE TO PUBLIC",
    )
    .unwrap();
    let owner = db.session("O");
    owner.run("INSERT INTO T VALUES ('O', 1), ('BOB', 2), ('ANN', 3)").unwrap();
    owner.commit().unwrap();
    assert_eq!(rows(&db.session("BOB"), "SELECT N FROM O.MINE"), expected(&["2"]));
    assert_eq!(rows(&db.session("ANN"), "SELECT N FROM O.MINE"), expected(&["3"]));
    assert_eq!(rows(&owner, "SELECT N FROM MINE"), expected(&["1"]));
    let bob = db.session("BOB");
    assert_eq!(exec(&bob, "INSERT INTO O.MINE VALUES ('BOB', 4)"), Ok(Completion::Success));
    assert_eq!(exec(&bob, "INSERT INTO O.MINE VALUES ('ANN', 5)"), Err(Code::Constraint), "not BOB's: WITH CHECK OPTION");
}
