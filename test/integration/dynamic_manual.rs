//! The examples of the manual's pages of dynamic SQL in SQL text
//! (`doc/sql/statements/`) that run in a program, through the Rust API,
//! as each page prints them: what each statement does, and the message of
//! each it refuses, as `SQLCODE (SQLSTATE): message`. The examples direct
//! SQL runs, the client runs as printed (`cli/test/manual.rs`).

use turborel::db::{Completion, Database, Session};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

/// Schema ANN, of the table STAFF of the pages: rows 1, Ada, 7200; 2,
/// Grace, 8100; and 3, NULL, 6900.
fn staff() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), SALARY DECIMAL(9,2))")
        .unwrap();
    let s = db.session("ANN");
    s.run("INSERT INTO STAFF VALUES (1, 'Ada', 7200), (2, 'Grace', 8100), (3, NULL, 6900)").unwrap();
    s.commit().unwrap();
    db
}

/// The program's parameters the pages declare: TEXT, a statement's text;
/// STMT and CUR, names; MY_ID, RAISE, FLOOR and N, numbers; MY_NAME and
/// its indicator NAME_IND; WHAT, a field's text.
fn program() -> Params {
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let name = DataType::Varying { length: 128 };
    Params::new()
        .declare("TEXT", DataType::Varying { length: 200 }, Value::Null)
        .declare("STMT", name, Value::Null)
        .declare("CUR", name, Value::Null)
        .declare("MY_ID", integer, Value::Exact(Decimal::new(1, 0)))
        .declare("RAISE", integer, Value::Exact(Decimal::new(100, 0)))
        .declare("FLOOR", integer, Value::Exact(Decimal::new(7000, 0)))
        .declare("N", integer, Value::Null)
        .declare("MY_NAME", DataType::Varying { length: 30 }, Value::Null)
        .indicator("NAME_IND", 0)
        .declare("WHAT", DataType::Character { length: 128 }, Value::Null)
}

/// `p`'s parameter `name` set to the string `s`.
fn set(p: &mut Params, name: &str, s: &str) {
    p.set(name, Value::Char(s.to_owned()));
}

/// `p`'s parameter `name` set to the number `n`.
fn number(p: &mut Params, name: &str, n: i128) {
    p.set(name, Value::Exact(Decimal::new(n, 0)));
}

/// The message `sql`, run with `p`, is refused with, as the pages print
/// it.
fn refused(s: &Session, sql: &str, p: &mut Params) -> String {
    let e = s.execute(sql, p).expect_err(sql);
    format!("{} ({}): {}", e.sqlcode(), e.sqlstate(), e.described())
}

/// The rows of `sql`, joined.
fn rows(s: &Session, sql: &str) -> String {
    let mut c = s.open_cursor(sql).unwrap();
    let mut all = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        all.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "));
    }
    all.join("; ")
}

/// The value of `p`'s parameter `name`, shown.
fn value(p: &Params, name: &str) -> String {
    p.value(name).to_string().trim_end().to_owned()
}

/// The statements and parameters of PREPARE's page: RAISE_ONE, and the
/// GLOBAL statement NAME_OF, which STMT names.
fn prepared(s: &Session, p: &mut Params) {
    set(p, "TEXT", "UPDATE STAFF SET SALARY = SALARY + ? WHERE ID = ?");
    s.execute("PREPARE RAISE_ONE FROM TEXT", p).unwrap();
    set(p, "STMT", "NAME_OF");
    set(p, "TEXT", "SELECT NAME FROM STAFF WHERE ID = ?");
    s.execute("PREPARE GLOBAL STMT FROM TEXT", p).unwrap();
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of PREPARE's page
/// run as printed [ISO/IEC 9075:1992 §17.6].
#[test]
fn the_prepare_page_runs_as_printed() {
    let db = staff();
    let s = db.session("ANN");
    let mut p = program();
    prepared(&s, &mut p);
    s.execute("EXECUTE RAISE_ONE USING RAISE, MY_ID", &mut p).unwrap();
    assert_eq!(rows(&s, "SELECT SALARY FROM STAFF WHERE ID = 1"), "7300.00");
    s.execute("EXECUTE GLOBAL STMT INTO MY_NAME INDICATOR NAME_IND USING MY_ID", &mut p).unwrap();
    assert_eq!(value(&p, "MY_NAME"), "Ada");
    assert_eq!(
        refused(&s, "PREPARE GONE FROM 'DELETE FROM STAFF'", &mut p),
        "-103 (2A000): an <SQL statement variable> is not a <literal> [ISO/IEC 9075:1992 §17.6 SR1]"
    );
    set(&mut p, "TEXT", "DELETE FROM STAF");
    assert_eq!(refused(&s, "PREPARE GONE FROM TEXT", &mut p), "-103 (37000): table ANN.STAF is not defined [X3.135-1986 §5.4 SR5]");
    set(&mut p, "TEXT", "SELECT ? FROM STAFF");
    assert_eq!(
        refused(&s, "PREPARE GONE FROM TEXT", &mut p),
        "-103 (37000): a <dynamic parameter specification> here takes no type from where it stands [ISO/IEC 9075:1992 §17.6 GR4]"
    );
    set(&mut p, "STMT", "TWO WORDS");
    assert_eq!(
        refused(&s, "PREPARE STMT FROM TEXT", &mut p),
        "-921 (26000): 'TWO WORDS' is not an <identifier> [ISO/IEC 9075:1992 §17.6 GR8]"
    );
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of EXECUTE's page
/// run as printed [ISO/IEC 9075:1992 §17.10].
#[test]
fn the_execute_page_runs_as_printed() {
    let db = staff();
    let s = db.session("ANN");
    let mut p = program();
    prepared(&s, &mut p);
    assert_eq!(s.execute("EXECUTE RAISE_ONE USING RAISE, MY_ID", &mut p).unwrap(), Completion::Success);
    assert_eq!(rows(&s, "SELECT SALARY FROM STAFF WHERE ID = 1"), "7300.00");
    number(&mut p, "MY_ID", 3);
    s.execute("EXECUTE GLOBAL STMT INTO MY_NAME INDICATOR NAME_IND USING MY_ID", &mut p).unwrap();
    assert_eq!(p.indicator_value("NAME_IND"), Some(-1));
    number(&mut p, "MY_ID", 9);
    assert_eq!(s.execute("EXECUTE GLOBAL STMT INTO MY_NAME INDICATOR NAME_IND USING MY_ID", &mut p).unwrap(), Completion::NoData);
    set(&mut p, "TEXT", "SELECT NAME FROM STAFF WHERE ID >= ?");
    s.execute("PREPARE MANY FROM TEXT", &mut p).unwrap();
    set(&mut p, "TEXT", "SELECT NAME FROM STAFF ORDER BY NAME");
    s.execute("PREPARE ORDERED FROM TEXT", &mut p).unwrap();
    number(&mut p, "MY_ID", 1);
    let refusals = [
        ("EXECUTE GONE", "-921 (26000): no statement GONE is prepared [ISO/IEC 9075:1992 §17.10 GR1]"),
        ("EXECUTE RAISE_ONE", "-921 (07004): the statement has dynamic parameters, and no <using clause> [ISO/IEC 9075:1992 §17.10 GR4]"),
        ("EXECUTE RAISE_ONE USING RAISE", "-921 (07001): 1 values for 2 <dynamic parameter specification>s [ISO/IEC 9075:1992 §17.9 GR5a]"),
        (
            "EXECUTE GLOBAL STMT USING MY_ID",
            "-921 (07007): a <dynamic single row select statement> needs a <result using clause> [ISO/IEC 9075:1992 §17.10 GR5]",
        ),
        (
            "EXECUTE MANY INTO MY_NAME INDICATOR NAME_IND USING MY_ID",
            "-811 (21000): the result has more than one row [ISO/IEC 9075:1992 §13.5 GR2a]",
        ),
        (
            "EXECUTE ORDERED INTO MY_NAME INDICATOR NAME_IND",
            "-921 (07003): a <cursor specification> of more than a <query specification> is opened, not executed [ISO/IEC 9075:1992 §17.10 GR2]",
        ),
    ];
    for (sql, message) in refusals {
        assert_eq!(refused(&s, sql, &mut p), message, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of GET DESCRIPTOR's
/// page run as printed [ISO/IEC 9075:1992 §17.4].
#[test]
fn the_get_descriptor_page_runs_as_printed() {
    let db = staff();
    let s = db.session("ANN");
    let mut p = program();
    set(&mut p, "MY_NAME", "Ada");
    s.execute("ALLOCATE DESCRIPTOR 'ROW' WITH MAX 3", &mut p).unwrap();
    s.execute("SET DESCRIPTOR 'ROW' COUNT = 1", &mut p).unwrap();
    s.execute("SET DESCRIPTOR 'ROW' VALUE 1 TYPE = 12, LENGTH = 30, DATA = MY_NAME", &mut p).unwrap();
    s.execute("GET DESCRIPTOR 'ROW' N = COUNT", &mut p).unwrap();
    assert_eq!(value(&p, "N"), "1");
    s.execute("GET DESCRIPTOR 'ROW' VALUE 1 N = TYPE, WHAT = CHARACTER_SET_NAME", &mut p).unwrap();
    assert_eq!((value(&p, "N"), value(&p, "WHAT")), ("12".to_owned(), "SQL_TEXT".to_owned()));
    s.execute("GET DESCRIPTOR 'ROW' VALUE 1 TEXT = DATA", &mut p).unwrap();
    assert_eq!(value(&p, "TEXT"), "Ada");
    assert_eq!(s.execute("GET DESCRIPTOR 'ROW' VALUE 2 N = TYPE", &mut p).unwrap(), Completion::NoData);
    s.execute("SET DESCRIPTOR 'ROW' VALUE 1 INDICATOR = -1", &mut p).unwrap();
    let refusals = [
        (
            "GET DESCRIPTOR 'ROW' VALUE 1 TEXT = DATA",
            "-301 (22002): INDICATOR is negative, and DATA is got without it [ISO/IEC 9075:1992 §17.4 GR5]",
        ),
        (
            "GET DESCRIPTOR 'ROW' VALUE 1 MY_ID = DATA",
            "-301 (22005): NUMERIC(9,0) does not match the type the item describes [ISO/IEC 9075:1992 §17.4 GR2, §17.1 SR2]",
        ),
        (
            "GET DESCRIPTOR 'ROW' VALUE 4 N = TYPE",
            "-921 (07009): item 4 is not of 1 to 3, the area's occurrences [ISO/IEC 9075:1992 §17.4 GR3]",
        ),
        (
            "GET DESCRIPTOR 'ROW' VALUE 1 WHAT = TYPE",
            "-103 (2A000): the target of TYPE is not of its type [ISO/IEC 9075:1992 §17.4 SR3, Table 17]",
        ),
        ("GET DESCRIPTOR 'NO_ROW' N = COUNT", "-921 (33000): no SQL descriptor area 'NO_ROW' is allocated [ISO/IEC 9075:1992 §17.4 GR1]"),
    ];
    for (sql, message) in refusals {
        assert_eq!(refused(&s, sql, &mut p), message, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of DESCRIBE's page
/// run as printed [ISO/IEC 9075:1992 §17.8, §17.9].
#[test]
fn the_describe_page_runs_as_printed() {
    let db = staff();
    let s = db.session("ANN");
    let mut p = program();
    set(&mut p, "TEXT", "SELECT ID, NAME, SALARY * 2 FROM STAFF WHERE ID = ?");
    s.execute("PREPARE PAY FROM TEXT", &mut p).unwrap();
    s.execute("ALLOCATE DESCRIPTOR 'PAY_ROW' WITH MAX 3", &mut p).unwrap();
    s.execute("DESCRIBE OUTPUT PAY USING SQL DESCRIPTOR 'PAY_ROW'", &mut p).unwrap();
    let field = |s: &Session, n: i32, f: &str, target: &str| {
        let mut q = program();
        s.execute(&format!("GET DESCRIPTOR 'PAY_ROW' VALUE {n} {target} = {f}"), &mut q).unwrap();
        value(&q, target)
    };
    let mut items = Vec::new();
    for n in 1..=3 {
        let numbers: Vec<String> = ["TYPE", "PRECISION", "SCALE", "LENGTH", "OCTET_LENGTH", "NULLABLE", "UNNAMED"]
            .iter()
            .map(|f| format!("{f} {}", field(&s, n, f, "N")))
            .collect();
        items.push(format!(
            "{}; NAME {}; {}",
            numbers.join(", "),
            field(&s, n, "NAME", "WHAT"),
            field(&s, n, "CHARACTER_SET_NAME", "WHAT")
        ));
    }
    assert_eq!(
        items,
        [
            "TYPE 4, PRECISION 10, SCALE 0, LENGTH 0, OCTET_LENGTH 0, NULLABLE 0, UNNAMED 0; NAME ID; ",
            "TYPE 12, PRECISION 0, SCALE 0, LENGTH 30, OCTET_LENGTH 120, NULLABLE 1, UNNAMED 0; NAME NAME; SQL_TEXT",
            "TYPE 2, PRECISION 18, SCALE 2, LENGTH 0, OCTET_LENGTH 0, NULLABLE 1, UNNAMED 1; NAME ; ",
        ]
    );
    s.execute("SET DESCRIPTOR 'PAY_ROW' VALUE 2 TYPE = 12, LENGTH = 30", &mut p).unwrap();
    number(&mut p, "MY_ID", 2);
    s.execute("EXECUTE PAY INTO SQL DESCRIPTOR 'PAY_ROW' USING MY_ID", &mut p).unwrap();
    s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 2 MY_NAME = DATA, N = RETURNED_LENGTH", &mut p).unwrap();
    assert_eq!((value(&p, "MY_NAME"), value(&p, "N")), ("Grace".to_owned(), "5".to_owned()));
    s.execute("DESCRIBE INPUT PAY USING SQL DESCRIPTOR 'PAY_ROW'", &mut p).unwrap();
    s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 1 N = TYPE", &mut p).unwrap();
    assert_eq!(value(&p, "N"), "4");
    s.execute("ALLOCATE DESCRIPTOR 'SMALL' WITH MAX 2", &mut p).unwrap();
    let warned = s.execute("DESCRIBE PAY USING SQL DESCRIPTOR 'SMALL'", &mut p).unwrap();
    assert_eq!(warned.sqlstate().as_str(), "01005");
    s.execute("GET DESCRIPTOR 'SMALL' N = COUNT", &mut p).unwrap();
    assert_eq!(value(&p, "N"), "3");
    let refusals = [
        ("DESCRIBE GONE USING SQL DESCRIPTOR 'PAY_ROW'", "-921 (26000): no statement GONE is prepared [ISO/IEC 9075:1992 §17.8 GR1]"),
        (
            "DESCRIBE PAY USING SQL DESCRIPTOR 'NO_ROW'",
            "-921 (33000): no SQL descriptor area 'NO_ROW' is allocated [ISO/IEC 9075:1992 §17.9 GR2]",
        ),
        (
            "DESCRIBE PAY INTO SQL DESCRIPTOR 'PAY_ROW'",
            "-103 (2A000): INTO is of a <dynamic fetch statement> or a <result using clause> alone [ISO/IEC 9075:1992 §17.9 SR1]",
        ),
    ];
    for (sql, message) in refusals {
        assert_eq!(refused(&s, sql, &mut p), message, "{sql}");
    }
}

/// The cursor of ALLOCATE CURSOR's page, RICH, which CUR names, of the
/// GLOBAL statement BY_PAY, open and on Ada's row.
fn rich(s: &Session, p: &mut Params) {
    set(p, "STMT", "BY_PAY");
    set(p, "TEXT", "SELECT ID, NAME FROM STAFF WHERE SALARY > ? FOR UPDATE OF SALARY");
    s.execute("PREPARE GLOBAL STMT FROM TEXT", p).unwrap();
    set(p, "CUR", "RICH");
    s.execute("ALLOCATE CUR CURSOR FOR GLOBAL STMT", p).unwrap();
    s.execute("OPEN CUR USING FLOOR", p).unwrap();
    s.execute("FETCH CUR INTO MY_ID, MY_NAME INDICATOR NAME_IND", p).unwrap();
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of ALLOCATE
/// CURSOR's page, and of DEALLOCATE PREPARE's, run as printed [ISO/IEC
/// 9075:1992 §17.7, §17.13-§17.18].
#[test]
fn the_allocate_cursor_and_deallocate_prepare_pages_run_as_printed() {
    let db = staff();
    let s = db.session("ANN");
    let mut p = program();
    rich(&s, &mut p);
    assert_eq!((value(&p, "MY_ID"), value(&p, "MY_NAME")), ("1".to_owned(), "Ada".to_owned()));
    s.execute("UPDATE STAFF SET SALARY = SALARY + 100 WHERE CURRENT OF CUR", &mut p).unwrap();
    s.execute("FETCH CUR INTO MY_ID, MY_NAME INDICATOR NAME_IND", &mut p).unwrap();
    assert_eq!((value(&p, "MY_ID"), value(&p, "MY_NAME")), ("2".to_owned(), "Grace".to_owned()));
    let refusals = [
        ("ALLOCATE CUR CURSOR FOR GLOBAL STMT", "-921 (34000): cursor 'RICH' is allocated already [ISO/IEC 9075:1992 §17.13 GR5]"),
        ("ALLOCATE GLOBAL CUR CURSOR FOR STMT", "-921 (26000): no statement BY_PAY is prepared [ISO/IEC 9075:1992 §17.13 GR1]"),
        ("OPEN GLOBAL CUR USING FLOOR", "-921 (34000): no cursor 'RICH' is allocated [ISO/IEC 9075:1992 §17.14 GR2]"),
        (
            "ALLOCATE 'RICH' CURSOR FOR GLOBAL STMT",
            "-103 (2A000): an <extended statement name> or <extended cursor name> is not a <literal> [ISO/IEC 9075:1992 §5.4 SR13]",
        ),
        ("DEALLOCATE PREPARE GLOBAL STMT", "-501 (24000): statement BY_PAY is of cursor RICH, which is open [ISO/IEC 9075:1992 §17.7 GR2]"),
    ];
    for (sql, message) in refusals {
        assert_eq!(refused(&s, sql, &mut p), message, "{sql}");
    }
    s.execute("CLOSE CUR", &mut p).unwrap();
    prepared(&s, &mut p);
    set(&mut p, "STMT", "BY_PAY");
    s.execute("DEALLOCATE PREPARE RAISE_ONE", &mut p).unwrap();
    s.execute("DEALLOCATE PREPARE GLOBAL STMT", &mut p).unwrap();
    let refusals = [
        ("EXECUTE RAISE_ONE USING RAISE, MY_ID", "-921 (26000): no statement RAISE_ONE is prepared [ISO/IEC 9075:1992 §17.10 GR1]"),
        ("DEALLOCATE PREPARE RAISE_ONE", "-921 (26000): no statement RAISE_ONE is prepared [ISO/IEC 9075:1992 §17.7 GR1]"),
        ("OPEN CUR USING FLOOR", "-921 (34000): no cursor 'RICH' is allocated [ISO/IEC 9075:1992 §17.14 GR2]"),
    ];
    for (sql, message) in refusals {
        assert_eq!(refused(&s, sql, &mut p), message, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of the dynamic
/// cursor of DECLARE CURSOR's page, and of OPEN's, FETCH's and CLOSE's,
/// run as printed [ISO/IEC 9075:1992 §17.12, §17.14-§17.16, §17.19].
#[test]
fn the_dynamic_cursor_examples_run_as_printed() {
    let db = staff();
    let s = db.session("ANN");
    let mut p = program();
    s.declare_cursor("DECLARE D SCROLL CURSOR FOR NAME_LIST").unwrap();
    assert_eq!(
        refused(&s, "OPEN D USING FLOOR", &mut p),
        "-921 (26000): no statement NAME_LIST is prepared [ISO/IEC 9075:1992 §17.14 GR1]"
    );
    set(&mut p, "TEXT", "SELECT ID, NAME FROM STAFF WHERE SALARY < ? ORDER BY ID");
    s.execute("PREPARE NAME_LIST FROM TEXT", &mut p).unwrap();
    assert_eq!(
        refused(&s, "OPEN D", &mut p),
        "-921 (07004): the statement has dynamic parameters, and no <using clause> [ISO/IEC 9075:1992 §17.14 GR3]"
    );
    s.execute("OPEN D USING FLOOR", &mut p).unwrap();
    s.execute("FETCH LAST FROM D INTO MY_ID, MY_NAME INDICATOR NAME_IND", &mut p).unwrap();
    assert_eq!((value(&p, "MY_ID"), p.indicator_value("NAME_IND")), ("3".to_owned(), Some(-1)));
    s.execute("ALLOCATE DESCRIPTOR 'ROW'", &mut p).unwrap();
    s.execute("DESCRIBE NAME_LIST USING SQL DESCRIPTOR 'ROW'", &mut p).unwrap();
    s.execute("SET DESCRIPTOR 'ROW' VALUE 1 TYPE = 2, PRECISION = 9", &mut p).unwrap();
    s.execute("FETCH FIRST FROM D INTO SQL DESCRIPTOR 'ROW'", &mut p).unwrap();
    s.execute("GET DESCRIPTOR 'ROW' VALUE 1 N = DATA", &mut p).unwrap();
    assert_eq!(value(&p, "N"), "3", "the first row below 7000");
    assert_eq!(
        refused(&s, "FETCH D USING MY_ID, MY_NAME", &mut p),
        "-103 (2A000): the <using clause> of a FETCH is of INTO [ISO/IEC 9075:1992 §17.15 SR2]"
    );
    assert_eq!(refused(&s, "OPEN D USING FLOOR", &mut p), "-501 (24000): cursor D is already open [X3.135-1986 §8.8 GR1]");
    s.execute("CLOSE D", &mut p).unwrap();
    set(&mut p, "CUR", "NONE");
    assert_eq!(refused(&s, "CLOSE CUR", &mut p), "-921 (34000): no cursor 'NONE' is allocated [ISO/IEC 9075:1992 §17.16 SR2]");
    s.declare_cursor("DECLARE U CURSOR FOR SELECT ID, SALARY FROM STAFF FOR UPDATE OF SALARY").unwrap();
    set(&mut p, "TEXT", "UPDATE SET SALARY = SALARY + ? WHERE CURRENT OF U");
    s.execute("PREPARE PAY_RISE FROM TEXT", &mut p).unwrap();
    s.execute("OPEN U", &mut p).unwrap();
    s.execute("FETCH U INTO MY_ID, N", &mut p).unwrap();
    s.execute("EXECUTE PAY_RISE USING RAISE", &mut p).unwrap();
    assert_eq!(rows(&s, "SELECT SALARY FROM STAFF WHERE ID = 1"), "7300.00");
}
