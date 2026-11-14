//! FETCH FIRST's PERCENT and WITH TIES [ISO/IEC 9075-2:2023 §7.17 GR7,
//! Features F866 and F867]: a percentage of the rows before OFFSET, of an
//! exact, approximate or `?` value, its ceiling kept; the peers of the last
//! row kept, by the keys' collation and null ordering; both together; in a
//! subquery, a view and ARRAY; each refusal; past the work area, and by
//! batches or row by row.

use turborel::db::Database;
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{Counted, code, exec, expected, rows, staff, state};

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR7b ii, NOTE 328, Feature
/// F866] FETCH FIRST n PERCENT keeps CEILING ( n * rows / 100.0E0 ) rows,
/// of the rows before OFFSET; past 100 percent, every row; of an exact or
/// approximate percentage, or of a `?`.
#[test]
fn a_percentage_keeps_its_ceiling_of_the_rows_7_17() {
    let (_db, a) = staff();
    let by_id = "SELECT ID FROM STAFF ORDER BY ID";
    assert_eq!(rows(&a, &format!("{by_id} FETCH FIRST 40 PERCENT ROWS ONLY")), expected(&["1", "2"]));
    assert_eq!(rows(&a, &format!("{by_id} FETCH FIRST 50 PERCENT ROWS ONLY")), expected(&["1", "2", "3"]), "CEILING(2.5)");
    assert_eq!(rows(&a, &format!("{by_id} FETCH NEXT 0.5 PERCENT ROW ONLY")), expected(&["1"]), "CEILING(0.025)");
    assert_eq!(rows(&a, &format!("{by_id} FETCH FIRST 2E1 PERCENT ROWS ONLY")), expected(&["1"]));
    assert_eq!(rows(&a, &format!("{by_id} OFFSET 3 ROWS FETCH FIRST 40 PERCENT ROWS ONLY")), expected(&["4", "5"]), "NOTE 328");
    assert_eq!(rows(&a, &format!("{by_id} FETCH FIRST 250 PERCENT ROWS ONLY")).unwrap().len(), 5);
    let q = a.prepare(&format!("{by_id} FETCH FIRST ? PERCENT ROWS ONLY")).unwrap();
    let mut opened = a.open_prepared(&q, &[Value::Approx(60.0)]).unwrap();
    let got: Vec<String> = std::iter::from_fn(|| opened.fetch().unwrap()).map(|r| r[0].to_string()).collect();
    assert_eq!(got, ["1", "2", "3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR7d i, Feature F867] WITH TIES
/// keeps the peers of the last row its count keeps, by every sort key,
/// nulls peers of each other, and of OFFSET, not the peers passed over.
#[test]
fn with_ties_keeps_the_peers_of_the_last_row_7_17() {
    let (_db, a) = staff();
    let by_dept = "SELECT NAME FROM STAFF ORDER BY DEPT NULLS LAST";
    assert_eq!(rows(&a, &format!("{by_dept} FETCH FIRST 1 ROW WITH TIES")).unwrap().len(), 2, "both of ENG");
    assert_eq!(rows(&a, &format!("{by_dept} FETCH FIRST 3 ROWS WITH TIES")).unwrap().len(), 4, "both of OPS");
    assert_eq!(rows(&a, &format!("{by_dept} OFFSET 1 ROW FETCH FIRST 1 ROW WITH TIES")).unwrap().len(), 1, "Ada passed over");
    assert_eq!(rows(&a, &format!("{by_dept}, ID FETCH FIRST 1 ROW WITH TIES")), expected(&["Ada"]), "of every key");
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF ORDER BY DEPT NULLS FIRST FETCH FIRST ROW WITH TIES"), expected(&["Barbara"]));
    a.run("CREATE TABLE TAGS (T VARCHAR(5), N INTEGER)").unwrap();
    a.run("INSERT INTO TAGS VALUES ('x', 1), ('X', 2), ('y', 3), (NULL, 4), (NULL, 5)").unwrap();
    let collated = "SELECT N FROM TAGS ORDER BY T COLLATE CASE_INSENSITIVE NULLS LAST, N FETCH FIRST 1 ROW WITH TIES";
    assert_eq!(rows(&a, collated), expected(&["1"]), "N is a key too");
    let collated = "SELECT N FROM TAGS ORDER BY T COLLATE CASE_INSENSITIVE NULLS LAST FETCH FIRST 1 ROW WITH TIES";
    assert_eq!(rows(&a, collated).unwrap().len(), 2, "x and X are peers of CASE_INSENSITIVE");
    let nulls = "SELECT N FROM TAGS ORDER BY T NULLS FIRST FETCH FIRST 1 ROW WITH TIES";
    assert_eq!(rows(&a, nulls).unwrap().len(), 2, "the null values are peers");
    // Both together.
    assert_eq!(rows(&a, &format!("{by_dept} FETCH FIRST 20 PERCENT ROWS WITH TIES")).unwrap().len(), 2);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17, Features F866, F867] PERCENT and
/// WITH TIES of a subquery, a derived table, a view and ARRAY's query.
#[test]
fn the_options_where_a_query_expression_is_7_17() {
    let (_db, a) = staff();
    let best = "SELECT NAME FROM STAFF WHERE DEPT IN (SELECT DEPT FROM STAFF ORDER BY SALARY DESC NULLS LAST FETCH FIRST 20 PERCENT ROWS ONLY) \
                ORDER BY ID";
    assert_eq!(rows(&a, best), expected(&["Ada", "Grace"]));
    let derived = "SELECT COUNT(*) FROM (SELECT * FROM STAFF ORDER BY DEPT NULLS LAST FETCH FIRST 3 ROWS WITH TIES) T";
    assert_eq!(rows(&a, derived), expected(&["4"]));
    a.run("CREATE VIEW FIRST_DEPT AS SELECT NAME, DEPT FROM STAFF ORDER BY DEPT FETCH FIRST ROW WITH TIES").unwrap();
    assert_eq!(rows(&a, "SELECT NAME FROM FIRST_DEPT ORDER BY NAME"), expected(&["Ada", "Grace"]));
    assert_eq!(exec(&a, "DELETE FROM FIRST_DEPT").err(), Some(Code::SyntaxRule), "§7.17 SR7");
    let array = "SELECT ARRAY(SELECT ID FROM STAFF ORDER BY DEPT DESC NULLS LAST FETCH FIRST ROW WITH TIES) FROM STAFF WHERE ID = 1";
    assert_eq!(rows(&a, array), expected(&["ARRAY[3, 5]"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR29, SR30, GR7c] a percentage
/// not a number, or WITH TIES of no ORDER BY, SQLCODE -103; a percentage
/// null, of 0, below, or of a count below 1, of no rows too, SQLSTATE
/// 2201W.
#[test]
fn the_options_refused_7_17() {
    let (_db, a) = staff();
    assert_eq!(code(&a, "SELECT ID FROM STAFF FETCH FIRST 'A' PERCENT ROWS ONLY"), Code::SyntaxRule, "SR29");
    assert_eq!(code(&a, "SELECT ID FROM STAFF FETCH FIRST 2 ROWS WITH TIES"), Code::SyntaxRule, "SR30");
    let nested = "SELECT ID FROM (SELECT ID FROM STAFF FETCH FIRST 2 ROWS WITH TIES) T ORDER BY ID";
    assert_eq!(code(&a, nested), Code::SyntaxRule, "SR30 of a query primary");
    for refused in ["0", "-10", "0.0E0"] {
        assert_eq!(state(&a, &format!("SELECT ID FROM STAFF FETCH FIRST {refused} PERCENT ROWS ONLY")), "2201W", "{refused}");
    }
    assert_eq!(state(&a, "SELECT ID FROM STAFF WHERE ID > 9 FETCH FIRST 50 PERCENT ROWS ONLY"), "2201W", "CEILING(0)");
    assert_eq!(state(&a, "SELECT ID FROM STAFF ORDER BY ID FETCH FIRST 0 ROWS WITH TIES"), "2201W");
    let q = a.prepare("SELECT ID FROM STAFF FETCH FIRST ? PERCENT ROWS ONLY").unwrap();
    let null = a.open_prepared(&q, &[Value::Null]).and_then(|mut o| o.fetch()).err().map(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(null.as_deref(), Some("2201W"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR7; RULES.md D38] a percentage's
/// rows held past MEMORY.WORK_BYTES spill, and PERCENT and WITH TIES keep
/// the same rows by batches and row by row; EXPLAIN shows them.
#[test]
fn the_options_past_the_work_area_and_both_ways_7_17() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER PRIMARY KEY, V INTEGER, P VARCHAR(40))").unwrap();
    let a = db.session("A");
    let n: u32 = 3000;
    let value = |i: u32| i.checked_rem(100).unwrap();
    let values: Vec<String> = (1..=n).map(|i| format!("({i}, {}, 'padding padding padding {i}')", value(i))).collect();
    a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    // 1 percent of 3000 rows, the 30 of V 0.
    let want: Vec<String> = (1..=n).filter(|&i| value(i) == 0).map(|i| i.to_string()).collect();
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        let percent = "SELECT I FROM M ORDER BY V, I FETCH FIRST 1 PERCENT ROWS ONLY";
        assert_eq!(rows(&a, percent).unwrap(), want, "batches {batches}");
        let tied = rows(&a, "SELECT P FROM M ORDER BY V FETCH FIRST 31 ROWS WITH TIES").unwrap();
        assert_eq!(tied.len(), 60, "both groups whole, batches {batches}");
        let past = rows(&a, "SELECT P FROM M ORDER BY I FETCH FIRST 50 PERCENT ROWS ONLY").unwrap();
        assert_eq!(past.len(), 1500, "batches {batches}");
    }
    assert!(spilled.taken() > 0, "the rows spilled");
    let plan = a.explain("SELECT I FROM M ORDER BY V FETCH FIRST 10 PERCENT ROWS WITH TIES", &Params::new()).unwrap();
    assert!(plan.contains("Limit fetch 10 percent with ties"), "{plan}");
}
