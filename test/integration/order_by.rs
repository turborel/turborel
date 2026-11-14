//! Sort keys that are value expressions [ISO/IEC 9075-2:2023 §7.17 SR26,
//! §10.10, Features E121-02 and E121-03]: of a simple table query, keys of
//! columns its select list leaves out, of expressions of them, alike a
//! select item, of its result's names, with COLLATE, DESC and NULLS, of a
//! window function, after a WITH clause, in ARRAY and through an updatable
//! cursor; of any other query expression, keys of its result alone; each
//! refusal; past the work area, and by batches or row by row.

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{Counted, code, exec, expected, rows};

/// Schema A: STAFF of four rows, one of no department and no salary, and
/// a name of a small letter first.
fn staff() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(20), DEPT CHAR(3), SALARY INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO STAFF VALUES (1, 'Ada', 'ENG', 7200), (2, 'grace', 'ENG', 8100), (3, 'Edsger', 'OPS', 5400), (4, 'Barbara', NULL, NULL)")
        .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26 d i 10, GR5; §10.10, Feature
/// E121-02] a sort key is a column the select list leaves out, sorted as
/// its direction and null ordering say, and taken away after the sort.
#[test]
fn a_column_left_out_sorts_the_rows_7_17() {
    let (_db, a) = staff();
    let sql = "SELECT NAME FROM STAFF ORDER BY SALARY DESC NULLS LAST";
    assert_eq!(rows(&a, sql), expected(&["grace", "Ada", "Edsger", "Barbara"]));
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF ORDER BY SALARY NULLS FIRST"), expected(&["Barbara", "Edsger", "Ada", "grace"]));
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF ORDER BY DEPT DESC NULLS LAST, ID DESC"), expected(&["Edsger", "grace", "Ada", "Barbara"]));
    // A qualified name, of a correlation name too, and a position.
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF ORDER BY STAFF.ID DESC"), expected(&["Barbara", "Edsger", "grace", "Ada"]));
    assert_eq!(rows(&a, "SELECT S.NAME FROM STAFF S ORDER BY S.ID DESC"), expected(&["Barbara", "Edsger", "grace", "Ada"]));
    assert_eq!(rows(&a, "SELECT NAME, ID FROM STAFF ORDER BY 2 DESC"), expected(&["Barbara,4", "Edsger,3", "grace,2", "Ada,1"]));
    // [§20.4] DESCRIBE shows the result's columns alone.
    assert_eq!(a.prepare(sql).unwrap().columns().len(), 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26, §10.10, Feature E121-03] a
/// sort key is a value expression, of the columns the select list names
/// or leaves out.
#[test]
fn a_value_expression_sorts_the_rows_7_17() {
    let (_db, a) = staff();
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF ORDER BY -ID"), expected(&["Barbara", "Edsger", "grace", "Ada"]));
    assert_eq!(rows(&a, "SELECT ID FROM STAFF ORDER BY CHAR_LENGTH(NAME) DESC, ID"), expected(&["4", "3", "2", "1"]));
    // 6200, 6100, 2400 and null.
    assert_eq!(
        rows(&a, "SELECT NAME FROM STAFF ORDER BY SALARY - 1000 * ID NULLS FIRST"),
        expected(&["Barbara", "Edsger", "grace", "Ada"])
    );
    let case = "SELECT ID FROM STAFF ORDER BY CASE WHEN DEPT = 'OPS' THEN 0 ELSE 1 END, ID DESC";
    assert_eq!(rows(&a, case), expected(&["3", "4", "2", "1"]));
    // Of a key that is one value for every row, the next key's order.
    assert_eq!(rows(&a, "SELECT NAME FROM STAFF ORDER BY ID * 0, NAME"), expected(&["Ada", "Barbara", "Edsger", "grace"]));
    let with = "WITH W AS (SELECT * FROM STAFF WHERE ID < 4) SELECT NAME FROM W ORDER BY SALARY DESC";
    assert_eq!(rows(&a, with), expected(&["grace", "Ada", "Edsger"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26 d i 6] a sort key alike an
/// item of the select list sorts by that item's column, computed once;
/// one alike none adds a column of its own.
#[test]
fn a_key_alike_a_select_item_sorts_by_its_column_7_17() {
    let (_db, a) = staff();
    let sql = "SELECT SALARY + ID FROM STAFF ORDER BY SALARY + ID DESC NULLS LAST";
    assert_eq!(rows(&a, sql), expected(&["8102", "7201", "5403", "NULL"]));
    let plan = a.explain(sql, &Params::new()).unwrap();
    let projections: Vec<&str> = plan.lines().map(str::trim).filter(|l| l.starts_with("Project")).collect();
    assert!(projections.iter().all(|l| *l == "Project (STAFF.SALARY + STAFF.ID)"), "computed once: {plan}");
    let other = "SELECT SALARY + ID FROM STAFF ORDER BY ID + SALARY DESC NULLS LAST";
    assert_eq!(rows(&a, other), expected(&["8102", "7201", "5403", "NULL"]));
    let plan = a.explain(other, &Params::new()).unwrap();
    assert!(plan.contains("(STAFF.ID + STAFF.SALARY)"), "a column of its own: {plan}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26 d i 6, d ii] in a sort key, a
/// name of a result's column is that column: of the result's columns
/// alone, the key is of its rows; with a column left out, of the select
/// item's value.
#[test]
fn a_name_of_the_result_is_its_column_in_a_key_7_17() {
    let (_db, a) = staff();
    let alone = "SELECT NAME AS N FROM STAFF ORDER BY CHAR_LENGTH(N), N";
    assert_eq!(rows(&a, alone), expected(&["Ada", "grace", "Edsger", "Barbara"]));
    // ID names the result's column, SALARY; S.ID the table's.
    let mixed = "SELECT SALARY AS ID FROM STAFF S ORDER BY ID - 1000 * S.ID NULLS LAST";
    assert_eq!(rows(&a, mixed), expected(&["5400", "8100", "7200", "NULL"]), "2400, 6100, 6200 and null");
    let named = "SELECT SALARY AS ID FROM STAFF ORDER BY ID DESC NULLS LAST";
    assert_eq!(rows(&a, named), expected(&["8100", "7200", "5400", "NULL"]));
    // Of DISTINCT and of groups, a key of the result alone.
    assert_eq!(rows(&a, "SELECT DISTINCT DEPT FROM STAFF ORDER BY CHAR_LENGTH(DEPT) NULLS FIRST, DEPT"), expected(&["NULL", "ENG", "OPS"]));
    let grouped = "SELECT DEPT, COUNT(*) AS C FROM STAFF GROUP BY DEPT ORDER BY -C, DEPT NULLS LAST";
    assert_eq!(rows(&a, grouped), expected(&["ENG,2", "OPS,1", "NULL,1"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10, §10.7, §9.15] a sort key of a
/// column left out sorts by its COLLATE, or the collation it has.
#[test]
fn a_key_left_out_sorts_by_its_collation_10_10() {
    let (_db, a) = staff();
    assert_eq!(rows(&a, "SELECT ID FROM STAFF ORDER BY NAME"), expected(&["1", "4", "3", "2"]));
    assert_eq!(rows(&a, "SELECT ID FROM STAFF ORDER BY NAME COLLATE CASE_INSENSITIVE DESC"), expected(&["2", "3", "4", "1"]));
    let upper = "SELECT ID FROM STAFF ORDER BY UPPER(NAME) COLLATE CASE_INSENSITIVE";
    assert_eq!(rows(&a, upper), expected(&["1", "4", "3", "2"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 SR2a, §7.17 SR26] a window function
/// is a sort key of a simple table query.
#[test]
fn a_window_function_sorts_the_rows_6_10() {
    let (_db, a) = staff();
    let sql = "SELECT NAME FROM STAFF ORDER BY ROW_NUMBER() OVER (ORDER BY SALARY DESC NULLS LAST)";
    assert_eq!(rows(&a, sql), expected(&["grace", "Ada", "Edsger", "Barbara"]));
    // MC/DC of "a window function, or a column not of the result": a
    // window function of the result's columns alone is computed with the
    // select list too.
    let of_result = "SELECT NAME FROM STAFF ORDER BY ROW_NUMBER() OVER (ORDER BY NAME DESC)";
    assert_eq!(rows(&a, of_result), expected(&["grace", "Edsger", "Barbara", "Ada"]));
    let partitioned = "SELECT ID FROM STAFF ORDER BY COUNT(*) OVER (PARTITION BY DEPT), ID DESC";
    assert_eq!(rows(&a, partitioned), expected(&["4", "3", "2", "1"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26 d i 1, d ii] a sort key of a
/// query expression that is not a simple table query is of its result's
/// columns.
#[test]
fn a_key_of_a_union_is_of_its_result_7_17() {
    let (_db, a) = staff();
    let union = "SELECT NAME FROM STAFF UNION SELECT DEPT AS NAME FROM STAFF ORDER BY CHAR_LENGTH(NAME) DESC NULLS LAST, NAME";
    assert_eq!(rows(&a, union), expected(&["Barbara", "Edsger", "grace", "Ada", "ENG", "OPS", "NULL"]));
    let values = "VALUES (1, 'b'), (2, 'a') ORDER BY 1 - 2";
    assert_eq!(code(&a, values), Code::SyntaxRule, "VALUES's columns have no names");
    assert_eq!(code(&a, "SELECT NAME FROM STAFF UNION SELECT DEPT FROM STAFF ORDER BY ID"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT NAME FROM STAFF UNION SELECT DEPT FROM STAFF ORDER BY CHAR_LENGTH(DEPT)"), Code::SyntaxRule);
    let windowed = "SELECT ID FROM STAFF UNION SELECT ID FROM STAFF ORDER BY ROW_NUMBER() OVER (ORDER BY ID)";
    assert_eq!(code(&a, windowed), Code::SyntaxRule, "§6.10 SR2a");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26b, d i 9 B, §6.14 SR1] a sort key
/// has a column reference and no query expression, set function or next
/// value expression; a key of a column left out is of a query neither
/// DISTINCT nor of a set function of its select list, and of a grouped
/// query is Feature F868.
#[test]
fn a_key_breaking_a_rule_is_refused_7_17() {
    let (_db, a) = staff();
    a.run("CREATE SEQUENCE Q").unwrap();
    for (sql, why) in [
        ("SELECT NAME FROM STAFF ORDER BY 1 + 1", "no column reference"),
        ("SELECT NAME FROM STAFF ORDER BY CAST(NULL AS INTEGER)", "no column reference"),
        ("SELECT DEPT FROM STAFF GROUP BY DEPT ORDER BY MAX(ID)", "a set function"),
        ("SELECT NAME FROM STAFF ORDER BY ID + MAX(ID)", "a set function beside a column"),
        ("SELECT NAME FROM STAFF ORDER BY (SELECT MAX(ID) FROM STAFF) + ID", "a query expression"),
        ("SELECT NAME FROM STAFF ORDER BY CASE WHEN ID IN (SELECT ID FROM STAFF) THEN 1 END", "a query expression"),
        ("SELECT NAME FROM STAFF ORDER BY NEXT VALUE FOR Q", "a next value expression"),
        ("SELECT DISTINCT NAME FROM STAFF ORDER BY ID", "DISTINCT"),
        ("SELECT DISTINCT NAME FROM STAFF ORDER BY CHAR_LENGTH(DEPT)", "DISTINCT"),
        ("SELECT COUNT(*) FROM STAFF ORDER BY ID", "a set function of the select list"),
        ("SELECT NAME FROM STAFF ORDER BY NOPE", "no such column"),
        ("SELECT NAME FROM STAFF ORDER BY 2", "no such position"),
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{why}: {sql}");
    }
    assert_eq!(code(&a, "SELECT DEPT FROM STAFF GROUP BY DEPT ORDER BY ID"), Code::Unsupported, "F868");
    assert_eq!(code(&a, "SELECT DEPT FROM STAFF GROUP BY DEPT ORDER BY CHAR_LENGTH(NAME)"), Code::Unsupported, "F868");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.49 GR3, §7.17 SR26, NOTE 207] the ORDER
/// BY of ARRAY(query) sorts its elements by keys of columns left out and
/// by value expressions.
#[test]
fn an_array_is_ordered_by_its_keys_6_49() {
    let (_db, a) = staff();
    let left_out = "SELECT ARRAY(SELECT NAME FROM STAFF ORDER BY SALARY DESC NULLS FIRST) FROM STAFF WHERE ID = 1";
    assert_eq!(rows(&a, left_out), expected(&["ARRAY['Barbara', 'grace', 'Ada', 'Edsger']"]));
    let both = "SELECT ARRAY(SELECT ID FROM STAFF ORDER BY CHAR_LENGTH(NAME) DESC, NAME COLLATE CASE_INSENSITIVE) FROM STAFF WHERE ID = 1";
    assert_eq!(rows(&a, both), expected(&["ARRAY[4, 3, 2, 1]"]));
    let union = "SELECT ARRAY(SELECT ID FROM STAFF UNION SELECT 9 AS ID FROM STAFF ORDER BY -ID) FROM STAFF WHERE ID = 1";
    assert_eq!(rows(&a, union), expected(&["ARRAY[9, 4, 3, 2, 1]"]));
    let correlated = "SELECT ID, ARRAY(SELECT X.ID FROM STAFF X WHERE X.ID <> S.ID ORDER BY X.ID * S.ID DESC) FROM STAFF S WHERE ID = 2";
    assert_eq!(rows(&a, correlated), expected(&["2,ARRAY[4, 3, 1]"]));
    let no_column = "SELECT ARRAY(SELECT ID FROM STAFF ORDER BY 1 + 1) FROM STAFF";
    assert_eq!(code(&a, no_column), Code::SyntaxRule);
}

/// The ID that FETCH from C gives, none of no data.
fn fetch(a: &Session, p: &mut Params) -> Option<String> {
    match a.execute("FETCH C INTO K_OUT", p).unwrap() {
        Completion::NoData => None,
        _ => Some(p.value("K_OUT").to_string()),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.3 SR5, Feature F833; ISO/IEC 9075:1992
/// §13.9 SR4] an updatable cursor ordered by a column left out, or a value
/// expression, moves in its order; a positioned UPDATE sets no column a key
/// reads, and sets one none reads.
#[test]
fn an_updatable_cursor_is_ordered_by_its_keys_13_9() {
    let (_db, a) = staff();
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let mut p = Params::new().declare("K_OUT", integer, Value::Null);
    a.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM STAFF WHERE ID < 4 ORDER BY SALARY - 100 * ID DESC FOR UPDATE").unwrap();
    a.execute("OPEN C", &mut p).unwrap();
    assert_eq!(fetch(&a, &mut p), Some("2".to_owned()));
    a.execute("UPDATE STAFF SET NAME = 'Hopper' WHERE CURRENT OF C", &mut p).unwrap();
    for set in ["SALARY = 0", "ID = 9"] {
        let refused = a.execute(&format!("UPDATE STAFF SET {set} WHERE CURRENT OF C"), &mut p).err().map(|e| e.code);
        assert_eq!(refused, Some(Code::SyntaxRule), "SR4: {set}");
    }
    assert_eq!(fetch(&a, &mut p), Some("1".to_owned()));
    a.execute("UPDATE STAFF SET DEPT = 'X' WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(fetch(&a, &mut p), Some("3".to_owned()));
    assert_eq!(fetch(&a, &mut p), None);
    a.execute("CLOSE C", &mut p).unwrap();
    assert_eq!(rows(&a, "SELECT NAME, DEPT FROM STAFF WHERE ID < 3 ORDER BY ID"), expected(&["Ada,X  ", "Hopper,ENG"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.3, §10.10; ISO/IEC 9075:1992 §13.9
/// SR4] an updatable cursor ordered by a collation opens, in its order, and
/// sets no column the key reads (once SQLCODE -910, the sort under a
/// projection not taken). One ordered by a window function is -900.
#[test]
fn an_updatable_cursor_is_ordered_by_a_collation_13_9() {
    let (_db, a) = staff();
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let mut p = Params::new().declare("K_OUT", integer, Value::Null);
    a.declare_cursor("DECLARE C CURSOR FOR SELECT ID, NAME FROM STAFF ORDER BY NAME COLLATE CASE_INSENSITIVE DESC FOR UPDATE").unwrap();
    a.execute("OPEN C", &mut p).unwrap();
    let mut q = p.clone().declare("N_OUT", DataType::character(20), Value::Null);
    a.execute("FETCH C INTO K_OUT, N_OUT", &mut q).unwrap();
    assert_eq!(q.value("K_OUT").to_string(), "2", "grace, last of the names but first of them DESC");
    let refused = a.execute("UPDATE STAFF SET NAME = 'G' WHERE CURRENT OF C", &mut q).err().map(|e| e.code);
    assert_eq!(refused, Some(Code::SyntaxRule), "SR4");
    a.execute("UPDATE STAFF SET SALARY = 1 WHERE CURRENT OF C", &mut q).unwrap();
    a.execute("CLOSE C", &mut q).unwrap();
    a.declare_cursor("DECLARE W CURSOR FOR SELECT ID FROM STAFF ORDER BY ROW_NUMBER() OVER (ORDER BY ID) FOR UPDATE").unwrap();
    assert_eq!(a.execute("OPEN W", &mut p).err().map(|e| e.code), Some(Code::Unsupported));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR26 d i 10; RULES.md D38] rows
/// sorted by a key left out past MEMORY.WORK_BYTES spill and come back in
/// order, row by row and by batches alike.
#[test]
fn keys_left_out_sort_past_the_work_area_7_17() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER, V INTEGER, P VARCHAR(40))").unwrap();
    let a = db.session("A");
    let n: u32 = 3000;
    let values: Vec<String> = (1..=n)
        .map(|i| format!("({i}, {}, 'padding padding padding {i}')", i.checked_mul(7).unwrap().checked_rem(1000).unwrap()))
        .collect();
    a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    let mut want: Vec<u32> = (1..=n).collect();
    want.sort_by_key(|&i| (i.checked_mul(7).unwrap().checked_rem(1000).unwrap(), std::cmp::Reverse(i)));
    let want: Vec<String> = want.iter().map(u32::to_string).collect();
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&a, "SELECT I FROM M ORDER BY V, I DESC").unwrap(), want, "batches {batches}");
        assert_eq!(rows(&a, "SELECT I FROM M ORDER BY V + 0, -I").unwrap(), want, "batches {batches}");
    }
    assert!(spilled.taken() > 0, "the sorts spilled");
}
