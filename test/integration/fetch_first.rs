//! ORDER BY, OFFSET and FETCH FIRST of any query expression [ISO/IEC
//! 9075-2:2023 §7.17, Features F850 to F865]: at the top of a query, of a
//! subquery, correlated too, of a query primary in parentheses, of a view,
//! of a WITH clause's query and body, of an INSERT's query and of ARRAY;
//! counts of literals, parameters and `?`, refused below their least or
//! null; a query of OFFSET or FETCH FIRST not updatable; a recursive query
//! of ORDER BY and OFFSET, FETCH FIRST refused there; past the work area,
//! and by batches or row by row. PERCENT and WITH TIES are in
//! `fetch_ties.rs`.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{Counted, code, exec, expected, rows};

/// Schema A: STAFF of five rows, one of no department and no salary;
/// DEPTS of three departments, keyed.
pub fn staff() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(20), DEPT CHAR(3), SALARY INTEGER) \
         CREATE TABLE DEPTS (DEPT CHAR(3) PRIMARY KEY, TITLE VARCHAR(20))",
    )
    .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO STAFF VALUES (1, 'Ada', 'ENG', 7200), (2, 'Grace', 'ENG', 8100), (3, 'Edsger', 'OPS', 5400), \
         (4, 'Barbara', NULL, NULL), (5, 'Alan', 'OPS', 6100)",
    )
    .unwrap();
    a.run("INSERT INTO DEPTS VALUES ('ENG', 'Engineering'), ('OPS', 'Operations'), ('HR', 'People')").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The SQLSTATE of the error `sql` raises as it opens or its rows are
/// read.
pub fn state(a: &Session, sql: &str) -> String {
    let read = a.open_cursor(sql).and_then(|mut c| {
        while c.fetch()?.is_some() {}
        Ok(())
    });
    read.err().map_or_else(|| panic!("no error: {sql}"), |e| e.sqlstate().as_str().to_owned())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR5, GR6, GR7,
/// Features F850, F857, F861] a query's rows sorted, then OFFSET's count
/// passed over and FETCH FIRST's count kept: FIRST or NEXT, ROW or ROWS,
/// one row of no count; all passed over past the last; of no ORDER BY, as
/// many rows.
#[test]
fn offset_and_fetch_first_keep_the_rows_in_order_7_17() {
    let (_db, a) = staff();
    let by_id = "SELECT NAME FROM STAFF ORDER BY ID";
    assert_eq!(rows(&a, &format!("{by_id} OFFSET 1 ROW FETCH FIRST 2 ROWS ONLY")), expected(&["Grace", "Edsger"]));
    assert_eq!(rows(&a, &format!("{by_id} OFFSET 3 ROWS")), expected(&["Barbara", "Alan"]));
    assert_eq!(rows(&a, &format!("{by_id} FETCH NEXT 2 ROW ONLY")), expected(&["Ada", "Grace"]));
    assert_eq!(rows(&a, &format!("{by_id} FETCH FIRST ROW ONLY")), expected(&["Ada"]), "GR7b iii");
    assert_eq!(rows(&a, &format!("{by_id} OFFSET 5 ROWS")), expected(&[]), "GR6c i");
    assert_eq!(rows(&a, &format!("{by_id} OFFSET 0 ROWS FETCH FIRST 9 ROWS ONLY")).unwrap().len(), 5);
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF ORDER BY SALARY DESC NULLS LAST FETCH FIRST 2 ROWS ONLY"), expected(&["Grace", "Ada"]));
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF FETCH FIRST 3 ROWS ONLY").unwrap().len(), 3);
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF OFFSET 4 ROWS").unwrap().len(), 1);
    // Of a set operation, a VALUES list and a WITH clause's body.
    assert_eq!(
        rows(&a, "SELECT DEPT FROM STAFF UNION SELECT DEPT FROM DEPTS ORDER BY 1 FETCH FIRST 2 ROWS ONLY"),
        expected(&["ENG", "HR "])
    );
    assert_eq!(rows(&a, "VALUES 3, 1, 2 ORDER BY 1 DESC OFFSET 1 ROW"), expected(&["2", "1"]));
    assert_eq!(
        rows(&a, "WITH W AS (SELECT * FROM STAFF) SELECT NAME FROM W ORDER BY SALARY FETCH FIRST 1 ROW ONLY"),
        expected(&["Edsger"])
    );
    // OFFSET, which SQL-92 does not reserve, still names a table.
    assert_eq!(rows(&a, "SELECT OFFSET.NAME FROM STAFF OFFSET ORDER BY OFFSET.ID FETCH FIRST 1 ROW ONLY"), expected(&["Ada"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17, Features F851, F858,
/// F862] the clauses of a subquery: a scalar one, correlated, run for each
/// row; under EXISTS and IN; of a derived table.
#[test]
fn a_subquery_is_ordered_and_limited_7_17() {
    let (_db, a) = staff();
    let top = "SELECT D.DEPT, (SELECT S.NAME FROM STAFF S WHERE S.DEPT = D.DEPT ORDER BY S.SALARY DESC FETCH FIRST 1 ROW ONLY) \
               FROM DEPTS D ORDER BY D.DEPT";
    assert_eq!(rows(&a, top), expected(&["ENG,Grace", "HR ,NULL", "OPS,Alan"]));
    let second = "SELECT D.DEPT FROM DEPTS D WHERE EXISTS (SELECT * FROM STAFF S WHERE S.DEPT = D.DEPT OFFSET 1 ROW) ORDER BY 1";
    assert_eq!(rows(&a, second), expected(&["ENG", "OPS"]));
    let best =
        "SELECT NAME FROM STAFF WHERE ID IN (SELECT ID FROM STAFF ORDER BY SALARY DESC NULLS LAST FETCH FIRST 2 ROWS ONLY) ORDER BY ID";
    assert_eq!(rows(&a, best), expected(&["Ada", "Grace"]));
    let derived = "SELECT T.NAME FROM (SELECT NAME, ID FROM STAFF ORDER BY ID DESC OFFSET 1 ROW FETCH FIRST 2 ROWS ONLY) T ORDER BY T.ID";
    assert_eq!(rows(&a, derived), expected(&["Edsger", "Barbara"]));
    // A scalar subquery of more than one row still raises.
    assert_eq!(state(&a, "SELECT (SELECT NAME FROM STAFF ORDER BY ID FETCH FIRST 2 ROWS ONLY) FROM DEPTS"), "21000");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17, Features F855, F856,
/// F863] a query primary in parentheses sorted and limited on its own,
/// then the query of it as its own clauses say; ARRAY's elements of the
/// rows its query keeps, in order [§6.49 NOTE 207]; an INSERT of the rows
/// its query keeps.
#[test]
fn a_query_primary_is_ordered_and_limited_7_17() {
    let (_db, a) = staff();
    let nested = "(SELECT NAME FROM STAFF ORDER BY ID FETCH FIRST 2 ROWS ONLY) UNION ALL \
                  (SELECT NAME FROM STAFF ORDER BY ID OFFSET 4 ROWS) ORDER BY 1 DESC";
    assert_eq!(rows(&a, nested), expected(&["Grace", "Alan", "Ada"]));
    let twice = "(SELECT ID FROM STAFF ORDER BY ID DESC FETCH FIRST 3 ROWS ONLY) ORDER BY ID FETCH FIRST 1 ROW ONLY";
    assert_eq!(rows(&a, twice), expected(&["3"]));
    let array = "SELECT ARRAY(SELECT NAME FROM STAFF ORDER BY SALARY DESC NULLS LAST OFFSET 1 ROW FETCH FIRST 2 ROWS ONLY) FROM DEPTS \
                 WHERE DEPT = 'HR'";
    assert_eq!(rows(&a, array), expected(&["ARRAY['Ada', 'Alan']"]));
    a.run("CREATE TABLE TOP2 (NAME VARCHAR(20))").unwrap();
    a.run("INSERT INTO TOP2 SELECT NAME FROM STAFF ORDER BY SALARY DESC NULLS LAST FETCH FIRST 2 ROWS ONLY").unwrap();
    assert_eq!(rows(&a, "SELECT NAME FROM TOP2 ORDER BY NAME"), expected(&["Ada", "Grace"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR7, SR11, §11.32,
/// Features F852, F859, F864] a view's query sorted and limited; a view of
/// ORDER BY alone updatable still, through which rows change; one of
/// OFFSET or FETCH FIRST not, nor a cursor of them FOR UPDATE.
#[test]
fn a_view_and_a_cursor_of_the_clauses_7_17() {
    let (_db, a) = staff();
    a.run("CREATE VIEW PAID AS SELECT NAME, SALARY FROM STAFF ORDER BY SALARY DESC NULLS LAST FETCH FIRST 3 ROWS ONLY").unwrap();
    assert_eq!(rows(&a, "SELECT NAME FROM PAID ORDER BY NAME"), expected(&["Ada", "Alan", "Grace"]));
    assert_eq!(
        rows(&a, "SELECT NAME FROM PAID WHERE SALARY < 8000 ORDER BY NAME"),
        expected(&["Ada", "Alan"]),
        "no filter below the limit"
    );
    assert_eq!(exec(&a, "UPDATE PAID SET SALARY = 1").err(), Some(Code::SyntaxRule), "§7.17 SR7");
    a.run("CREATE VIEW SORTED AS SELECT ID, SALARY FROM STAFF ORDER BY SALARY").unwrap();
    a.run("UPDATE SORTED SET SALARY = SALARY + 1 WHERE ID = 3").unwrap();
    assert_eq!(rows(&a, "SELECT SALARY FROM STAFF WHERE ID = 3"), expected(&["5401"]));
    a.run("CREATE VIEW SKIPPED AS SELECT ID, SALARY FROM STAFF OFFSET 1 ROW").unwrap();
    assert_eq!(exec(&a, "DELETE FROM SKIPPED").err(), Some(Code::SyntaxRule), "§7.17 SR7");
    a.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM STAFF FETCH FIRST 1 ROW ONLY FOR UPDATE").unwrap();
    assert!(a.execute("OPEN C", &mut Params::new()).is_err(), "§7.17 SR7");
    a.declare_cursor("DECLARE R CURSOR FOR SELECT ID FROM STAFF ORDER BY ID OFFSET 3 ROWS").unwrap();
    let mut p = Params::new().declare("K", DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }, Value::Null);
    a.execute("OPEN R", &mut p).unwrap();
    a.execute("FETCH R INTO K", &mut p).unwrap();
    assert_eq!(p.value("K").to_string(), "4");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR27, SR28, GR6b, GR7c,
/// Features F860, F865] a count of a parameter, or a `?`, its value read
/// as the rows are; one below its least or null SQLSTATE 2201X of OFFSET,
/// 2201W of FETCH FIRST, 0 rows too; one not an exact number of scale 0
/// SQLCODE -103.
#[test]
fn a_count_is_a_value_given_or_refused_7_17() {
    let (_db, a) = staff();
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let mut p = Params::new().declare("N", integer.clone(), Value::Exact(Decimal::new(2, 0))).declare("K", integer, Value::Null);
    a.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM STAFF ORDER BY ID OFFSET N ROWS FETCH FIRST 1 ROW ONLY").unwrap();
    a.execute("OPEN C", &mut p).unwrap();
    a.execute("FETCH C INTO K", &mut p).unwrap();
    assert_eq!(p.value("K").to_string(), "3");
    assert_eq!(a.execute("FETCH C INTO K", &mut p).unwrap(), turborel::db::Completion::NoData);
    a.execute("CLOSE C", &mut p).unwrap();
    let q = a.prepare("SELECT ID FROM STAFF ORDER BY ID OFFSET ? ROWS FETCH FIRST ? ROWS ONLY").unwrap();
    let int = |n: i128| Value::Exact(Decimal::new(n, 0));
    let mut opened = a.open_prepared(&q, &[int(3), int(5)]).unwrap();
    let got: Vec<String> = std::iter::from_fn(|| opened.fetch().unwrap()).map(|r| r[0].to_string()).collect();
    assert_eq!(got, ["4", "5"]);
    drop(opened);
    let failed = |values: &[Value]| a.open_prepared(&q, values).and_then(|mut o| o.fetch()).err().map(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(failed(&[int(-1), int(1)]).as_deref(), Some("2201X"));
    assert_eq!(failed(&[int(0), int(0)]).as_deref(), Some("2201W"));
    assert_eq!(failed(&[Value::Null, int(1)]).as_deref(), Some("2201X"));
    assert_eq!(failed(&[int(0), Value::Null]).as_deref(), Some("2201W"));
    assert_eq!(state(&a, "SELECT ID FROM STAFF FETCH FIRST 0 ROWS ONLY"), "2201W");
    assert_eq!(state(&a, "SELECT ID FROM STAFF OFFSET -2 ROWS"), "2201X");
    assert_eq!(code(&a, "SELECT ID FROM STAFF FETCH FIRST 1.5 ROWS ONLY"), Code::SyntaxRule, "SR28");
    assert_eq!(code(&a, "SELECT ID FROM STAFF OFFSET 'A' ROWS"), Code::SyntaxRule, "SR27");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3j ix 3, ix 4, GR2c
/// ii] a recursive query's ORDER BY and OFFSET, its fixpoint found as the
/// General Rules find it, each iteration of all its rows; FETCH FIRST of a
/// recursive query, or of a query primary in it of a reference to its
/// stratum, SQLCODE -103; of an operand of none, kept.
#[test]
fn a_recursive_query_of_the_clauses_7_17() {
    let (_db, a) = staff();
    let ordered = "WITH RECURSIVE N (I) AS (SELECT 1 FROM DEPTS WHERE DEPT = 'HR' UNION SELECT I + 1 FROM N WHERE I < 5 ORDER BY 1) \
                   SELECT I FROM N ORDER BY I";
    assert_eq!(rows(&a, ordered), expected(&["1", "2", "3", "4", "5"]));
    // Each evaluation, of the rows found so far, passes over its least
    // row: {2}, then {2, 4}, then {2, 4, 6}, which the next gives again.
    let skipped = "WITH RECURSIVE N (I) AS (VALUES 1, 2 UNION SELECT I + 2 FROM N WHERE I < 6 ORDER BY 1 OFFSET 1 ROW) \
                   SELECT I FROM N ORDER BY I";
    assert_eq!(rows(&a, skipped), expected(&["2", "4", "6"]));
    // OFFSET of a derived table of the work table: of all its rows each
    // time, not of those the last iteration added, which give 1, 2 and 3.
    let derived = "WITH RECURSIVE N (I) AS (VALUES 1, 2 UNION SELECT I + 1 FROM (SELECT I FROM N ORDER BY I OFFSET 1 ROW) X \
                   WHERE I < 5) SELECT I FROM N ORDER BY I";
    assert_eq!(rows(&a, derived), expected(&["1", "2", "3", "4", "5"]));
    // A name of the WITH list's is not of the union's result.
    assert_eq!(code(&a, &skipped.replace("ORDER BY 1 OFFSET", "ORDER BY I OFFSET")), Code::SyntaxRule);
    let fetched = "WITH RECURSIVE N (I) AS (VALUES 1 UNION ALL SELECT I + 1 FROM N WHERE I < 5 FETCH FIRST 3 ROWS ONLY) SELECT I FROM N";
    let e = a.open_cursor(fetched).err().unwrap();
    assert!(e.code == Code::SyntaxRule && e.to_string().contains("§7.17 SR3j ix 3"), "{e:?}");
    let nested = "WITH RECURSIVE N (I) AS (VALUES 1 UNION ALL (SELECT I + 1 FROM N WHERE I < 5 FETCH FIRST 3 ROWS ONLY)) SELECT I FROM N";
    let e = a.open_cursor(nested).err().unwrap();
    assert!(e.code == Code::SyntaxRule && e.to_string().contains("§7.17 SR3j ix 4"), "{e:?}");
    let anchored = "WITH RECURSIVE N (I) AS ((SELECT ID FROM STAFF ORDER BY ID FETCH FIRST 1 ROW ONLY) UNION ALL \
                    SELECT I + 1 FROM N WHERE I < 3) SELECT I FROM N ORDER BY I";
    assert_eq!(rows(&a, anchored), expected(&["1", "2", "3"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR6, GR7; RULES.md D38]
/// rows sorted past MEMORY.WORK_BYTES spill, and OFFSET and FETCH FIRST
/// keep the same of them by batches and row by row; a scan in an index's
/// order is limited without a sort.
#[test]
fn the_clauses_past_the_work_area_and_both_ways_7_17() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER PRIMARY KEY, V INTEGER, P VARCHAR(40))").unwrap();
    let a = db.session("A");
    let n: u32 = 3000;
    let value = |i: u32| i.checked_mul(7).unwrap().checked_rem(1000).unwrap();
    let values: Vec<String> = (1..=n).map(|i| format!("({i}, {}, 'padding padding padding {i}')", value(i))).collect();
    a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    let mut want: Vec<u32> = (1..=n).collect();
    want.sort_by_key(|&i| (value(i), i));
    let want: Vec<String> = want.iter().skip(1000).take(25).map(u32::to_string).collect();
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&a, "SELECT I FROM M ORDER BY V, I OFFSET 1000 ROWS FETCH FIRST 25 ROWS ONLY").unwrap(), want, "batches {batches}");
        let first: Vec<String> = (2991..=n).map(|i| i.to_string()).collect();
        assert_eq!(rows(&a, "SELECT I FROM M ORDER BY I OFFSET 2990 ROWS").unwrap(), first, "batches {batches}");
    }
    assert!(spilled.taken() > 0, "the sorts spilled");
    let plan = a.explain("SELECT I FROM M ORDER BY I FETCH FIRST 3 ROWS ONLY", &Params::new()).unwrap();
    let physical = plan.split("-- physical:").nth(1).unwrap_or_default();
    assert!(physical.contains("Limit fetch 3") && !physical.contains("Sort"), "{plan}");
}
