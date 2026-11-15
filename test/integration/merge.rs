//! MERGE [ISO/IEC 9075-2:2023 §14.12; F312, F313, F314]: matched rows of
//! the target updated or deleted and unmatched rows of the source inserted,
//! in one statement; the first applicable clause taking each row; the
//! source and every condition evaluated before any change; one source row
//! at most for a target row; the WHEN NOT MATCHED condition as printed; no
//! data only for an empty source; and what the diagnostics area says.

use turborel::db::{Completion, Database, Direct, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{failure, number, rows};

/// T as `merged` leaves it, ordered by K.
pub const BEFORE: [&str; 3] = ["1,10,a", "2,20,b", "3,30,c"];

/// The upsert of S into T: rows 2 and 3 matched and updated from both,
/// row 4 of S inserted.
pub const UPSERT: &str = "MERGE INTO T USING S ON T.K = S.K \
     WHEN MATCHED THEN UPDATE SET V = T.V + S.V, N = S.N \
     WHEN NOT MATCHED THEN INSERT (K, V, N) VALUES (S.K, S.V, S.N)";

/// T after `UPSERT`.
pub const UPSERTED: [&str; 4] = ["1,10,a", "2,220,B", "3,330,C", "4,400,D"];

/// Schema A: T (K INTEGER NOT NULL PRIMARY KEY, V INTEGER, N VARCHAR(8)
/// DEFAULT 'none') holding (1, 10, 'a'), (2, 20, 'b'), (3, 30, 'c'); S of
/// the same columns, no key, holding (2, 200, 'B'), (3, 300, 'C'), (4, 400,
/// 'D'); E of the same columns, empty. Committed.
pub fn merged() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V INTEGER, N VARCHAR(8) DEFAULT 'none')
           CREATE TABLE S (K INTEGER, V INTEGER, N VARCHAR(8))
           CREATE TABLE E (K INTEGER, V INTEGER, N VARCHAR(8))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10, 'a'), (2, 20, 'b'), (3, 30, 'c')").unwrap();
    a.run("INSERT INTO S VALUES (2, 200, 'B'), (3, 300, 'C'), (4, 400, 'D')").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of T, ordered by K.
pub fn target(s: &Session) -> Vec<String> {
    rows(s, "SELECT K, V, N FROM T ORDER BY K").unwrap()
}

/// The SQLSTATE `sql` ends with: 00000 if it succeeds, 02000 for no data.
pub fn state(s: &Session, sql: &str) -> String {
    match s.run(sql) {
        Ok(done) => done.sqlstate().as_str().to_owned(),
        Err(e) => e.sqlstate().as_str().to_owned(),
    }
}

/// The statement information item `item` of the last statement, as text
/// [ISO/IEC 9075-2:2023 §23.1 GR2].
pub fn statement(s: &Session, item: &str) -> String {
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let mut p = Params::new().declare("NUM", exact, Value::Null).declare("TXT", DataType::varying(128), Value::Null);
    let into = if item == "ROW_COUNT" { "NUM" } else { "TXT" };
    s.execute(&format!("GET DIAGNOSTICS {into} = {item}"), &mut p).unwrap();
    p.value(into).to_string().trim_end().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B III, GR6a ii 7; F312] one
/// MERGE updates the target rows the source matches and inserts the source
/// rows that match none; a SET clause reads the target row and the source
/// row both, by the table names or by correlation names, AS written or
/// not; a rollback undoes it all.
#[test]
fn matched_rows_updated_and_unmatched_rows_inserted_14_12() {
    let db = merged();
    let a = db.session("A");
    assert_eq!(a.run(UPSERT).unwrap(), Completion::Success);
    assert_eq!(target(&a), UPSERTED);
    a.rollback().unwrap();
    assert_eq!(target(&a), BEFORE, "rolled back");
    let named = "MERGE INTO T AS X USING S Y ON X.K = Y.K \
                 WHEN MATCHED THEN UPDATE SET V = X.V * 2 + Y.V \
                 WHEN NOT MATCHED THEN INSERT (N, K) VALUES (Y.N, Y.K)";
    assert_eq!(a.run(named).unwrap(), Completion::Success);
    assert_eq!(target(&a), ["1,10,a", "2,240,b", "3,360,c", "4,NULL,D"], "correlation names");
    a.commit().unwrap();
    assert_eq!(target(&db.session("A")), ["1,10,a", "2,240,b", "3,360,c", "4,NULL,D"], "committed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR18a, SR18b, SR18e, GR6a ii 7 A,
/// C, D] the implicit insert column list is every column of T in order;
/// each value is a source column, NULL, DEFAULT (the column's default) or
/// an expression; an explicit list in any order, the columns it leaves out
/// taking their defaults.
#[test]
fn insert_values_of_columns_null_default_and_expressions_14_12() {
    let db = merged();
    let a = db.session("A");
    let implicit = "MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN INSERT VALUES (S.K * 10, NULL, DEFAULT)";
    a.run(implicit).unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,20,b", "3,30,c", "40,NULL,none"], "SR18a");
    a.rollback().unwrap();
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN INSERT (K) VALUES (S.K + 1)").unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,20,b", "3,30,c", "5,NULL,none"], "the others defaulted");
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN INSERT (V, K) VALUES (S.V - 1, S.K)").unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,20,b", "3,30,c", "4,399,none", "5,NULL,none"], "in the list's order");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 A II, GR6a ii 5; F313,
/// F314] of several WHEN MATCHED clauses the first whose condition is true
/// takes the row, a later one true too never; likewise of several WHEN NOT
/// MATCHED clauses the first inserts the row, once; a row no clause's
/// condition holds of is left alone, matched or not.
#[test]
fn the_first_applicable_clause_takes_the_row_14_12() {
    let db = merged();
    let a = db.session("A");
    let several = "MERGE INTO T USING S ON T.K = S.K \
                   WHEN MATCHED AND S.V > 250 THEN UPDATE SET V = 1 \
                   WHEN MATCHED AND S.V > 100 THEN UPDATE SET V = 2 \
                   WHEN NOT MATCHED AND S.V > 100 THEN INSERT VALUES (S.K, 3, 'first') \
                   WHEN NOT MATCHED THEN INSERT VALUES (S.K, 4, 'second')";
    assert_eq!(a.run(several).unwrap(), Completion::Success);
    assert_eq!(target(&a), ["1,10,a", "2,2,b", "3,1,c", "4,3,first"]);
    a.rollback().unwrap();
    let some = "MERGE INTO T USING S ON T.K = S.K \
                WHEN MATCHED AND S.V > 250 THEN DELETE \
                WHEN NOT MATCHED AND S.V < 0 THEN INSERT VALUES (S.K, 0, 'x')";
    assert_eq!(a.run(some).unwrap(), Completion::Success);
    assert_eq!(target(&a), ["1,10,a", "2,20,b"], "row 2 and row 4 of S taken by no clause");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 A II] a target row is a
/// subject row of a WHEN MATCHED clause only if the ON condition and the
/// clause's condition are both true for it and some source row. MC/DC of
/// that conjunction, by a condition of the target row alone (T.V < 25):
/// row 2, both true, is updated; row 3, ON true and the condition false,
/// is not, so the condition decides; row 1, the condition true and ON
/// false for every source row, is not, so ON decides.
#[test]
fn a_subject_row_needs_on_and_the_clause_condition_true_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED AND T.V < 25 THEN UPDATE SET N = 'hit'").unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,20,hit", "3,30,c"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 7; F314] WHEN MATCHED THEN
/// DELETE deletes every subject row; among UPDATE clauses, before or after
/// them, it takes the rows its condition holds of and they the others.
#[test]
fn matched_rows_deleted_alone_and_among_updates_14_12() {
    let db = merged();
    let a = db.session("A");
    assert_eq!(a.run("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN DELETE").unwrap(), Completion::Success);
    assert_eq!(target(&a), ["1,10,a"]);
    a.rollback().unwrap();
    let first = "MERGE INTO T USING S ON T.K = S.K \
                 WHEN MATCHED AND S.V < 250 THEN DELETE \
                 WHEN MATCHED THEN UPDATE SET V = S.V \
                 WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V, S.N)";
    a.run(first).unwrap();
    assert_eq!(target(&a), ["1,10,a", "3,300,c", "4,400,D"], "DELETE first");
    a.rollback().unwrap();
    let last = "MERGE INTO T USING S ON T.K = S.K WHEN MATCHED AND S.V < 250 THEN UPDATE SET V = 0 WHEN MATCHED THEN DELETE";
    a.run(last).unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,0,b"], "DELETE last");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR5, GR6a i 1 A, GR6a i 1 B III 2,
/// GR6a ii 6] the source is evaluated before any row of T changes. The
/// target itself as the source, under another correlation name or in a
/// derived table, gives each row its predecessor's old value, not one
/// carried along; a SET value and an inserted value read T as it was; a
/// row inserted by an earlier clause is never a later WHEN MATCHED clause's
/// subject row.
#[test]
fn the_source_and_conditions_are_read_before_any_change_14_12() {
    let db = merged();
    let a = db.session("A");
    for source in ["T AS P", "(SELECT K, V FROM T) AS P (K, V)"] {
        let shifted = format!(
            "MERGE INTO T USING {source} ON T.K = P.K + 1 \
             WHEN MATCHED THEN UPDATE SET V = P.V \
             WHEN NOT MATCHED THEN INSERT (K, V) VALUES (P.K + 1, P.V)"
        );
        a.run(&shifted).unwrap();
        assert_eq!(target(&a), ["1,10,a", "2,10,b", "3,20,c", "4,30,none"], "{source}");
        a.rollback().unwrap();
    }
    let read = "MERGE INTO T USING S ON T.K = S.K \
                WHEN MATCHED THEN UPDATE SET V = (SELECT SUM(V) FROM T) \
                WHEN NOT MATCHED THEN INSERT (K, V) VALUES (S.K, (SELECT COUNT(*) FROM T))";
    a.run(read).unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,60,b", "3,60,c", "4,3,none"], "T as it was");
    a.rollback().unwrap();
    // Correlated, so evaluated for each row: row 3's sum is of row 2 as it was.
    let running = "MERGE INTO T USING S ON T.K = S.K \
                   WHEN MATCHED THEN UPDATE SET V = (SELECT SUM(X.V) FROM T AS X WHERE X.K <= T.K)";
    a.run(running).unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,30,b", "3,60,c"], "each sum of T as it was");
    a.rollback().unwrap();
    let inserted = "MERGE INTO T USING S ON T.K = S.K \
                    WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V, S.N) \
                    WHEN MATCHED THEN DELETE";
    a.run(inserted).unwrap();
    assert_eq!(target(&a), ["1,10,a", "4,400,D"], "row 4, inserted, not deleted");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B I, II; X3.135-1986 §3.3]
/// two source rows matching one target row for its clause raise a
/// cardinality violation (SQLCODE -811, SQLSTATE 21000), for UPDATE and for
/// DELETE, and the statement has no effect, not the rows it updated or
/// inserted for others; the matching rows are those for which the clause's
/// condition is true too, so two source rows of which one satisfies it are
/// none, nor two that the first clause and a later one take each.
#[test]
fn two_source_rows_for_one_target_row_raise_21000_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("INSERT INTO S VALUES (2, 222, 'X')").unwrap();
    a.commit().unwrap();
    assert_eq!(failure(a.run(UPSERT)), (Code::Cardinality, "21000".to_owned()), "UPDATE");
    assert_eq!(target(&a), BEFORE, "no effect");
    let delete = "MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN DELETE";
    assert_eq!(failure(a.run(delete)), (Code::Cardinality, "21000".to_owned()), "DELETE");
    assert_eq!(target(&a), BEFORE, "no effect");
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED AND S.V = 222 THEN UPDATE SET V = S.V").unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,222,b", "3,30,c"], "one of the two satisfies the condition");
    a.rollback().unwrap();
    let each = "MERGE INTO T USING S ON T.K = S.K \
                WHEN MATCHED AND S.V = 222 THEN UPDATE SET N = 'one' \
                WHEN MATCHED THEN UPDATE SET N = 'two'";
    a.run(each).unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,20,one", "3,30,two"], "row 2 the first clause's alone");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a ii 1, 2, 3] a source row is
/// inserted by WHEN NOT MATCHED AND c only if no target row makes `ON OR
/// NOT c` true, as printed. Into an empty target every source row is
/// inserted whatever c; a row for which c is unknown is inserted, one for
/// which it is false not; c may read the target row, the row inserted only
/// if c is false for none. MC/DC of the flag, no target row making ON true
/// and none making NOT c true: row 4 of S, neither, is inserted; rows 2
/// and 3, ON true, c true, are not, so ON decides; row 6, ON false for
/// every row, c false, is not, so c decides.
#[test]
fn the_not_matched_condition_as_printed_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("INSERT INTO S VALUES (5, NULL, 'E'), (6, 50, 'F')").unwrap();
    a.commit().unwrap();
    let flagged = "MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED AND S.V > 100 THEN INSERT VALUES (S.K, S.V, S.N)";
    a.run(flagged).unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,20,b", "3,30,c", "4,400,D", "5,NULL,E"], "row 5 unknown, row 6 false");
    a.rollback().unwrap();
    a.run("INSERT INTO S VALUES (7, 15, 'G')").unwrap();
    let against = "MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED AND S.V > T.V THEN INSERT VALUES (S.K, S.V, S.N)";
    a.run(against).unwrap();
    let read = ["1,10,a", "2,20,b", "3,30,c", "4,400,D", "5,NULL,E", "6,50,F"];
    assert_eq!(target(&a), read, "row 7 not: 15 > 20 is false");
    a.rollback().unwrap();
    a.run("DELETE FROM T").unwrap();
    let never = "MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED AND S.V < 0 THEN INSERT VALUES (S.K, S.V, S.N)";
    a.run(never).unwrap();
    assert_eq!(target(&a), ["2,200,B", "3,300,C", "4,400,D", "5,NULL,E", "6,50,F"], "an empty target");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR5, GR6a ii 1] an ON condition
/// whose subquery reads both the target row and the source row matches as
/// it says, in the join and in a WHEN NOT MATCHED clause's flag alike, by
/// batches and row by row: row 2 of S matched, as E has its key with
/// another value, row 3 not, as E lacks its key, and refused by its
/// clause's condition, row 4 inserted.
#[test]
fn an_on_subquery_reading_both_sides_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("INSERT INTO E VALUES (2, 0, 'x'), (4, 0, 'y')").unwrap();
    let both = "MERGE INTO T USING S ON T.K = S.K AND EXISTS (SELECT * FROM E WHERE E.K = S.K AND E.V <> T.V) \
                WHEN MATCHED THEN UPDATE SET N = 'm' \
                WHEN NOT MATCHED AND S.V > 350 THEN INSERT VALUES (S.K, S.V, S.N)";
    for batches in ["ON", "OFF"] {
        a.run(&format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        a.run(both).unwrap();
        assert_eq!(target(&a), ["1,10,a", "2,20,m", "3,30,c", "4,400,D"], "batches {batches}");
        a.run("DELETE FROM T WHERE K = 4").unwrap();
        a.run("UPDATE T SET N = 'b' WHERE K = 2").unwrap();
    }
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR8; §23.1 GR2f ii] an empty source
/// is no data (SQLCODE 100, SQLSTATE 02000), changing nothing, ROW_COUNT 0;
/// a source of rows that changes no row completes successfully, ROW_COUNT
/// the pairs ON matches, of WHEN MATCHED clauses alone \[GR2f ii 1\].
/// GR8's one condition: the source empty, a table's or a derived table's,
/// or not.
#[test]
fn an_empty_source_alone_is_no_data_14_12() {
    let db = merged();
    let a = db.session("A");
    let empty = "MERGE INTO T USING E ON T.K = E.K WHEN MATCHED THEN DELETE WHEN NOT MATCHED THEN INSERT VALUES (E.K, E.V, E.N)";
    assert_eq!(a.run(empty).unwrap(), Completion::NoData);
    assert_eq!(statement(&a, "ROW_COUNT"), "0");
    assert_eq!(state(&a, empty), "02000");
    let filtered = "MERGE INTO T USING (SELECT * FROM S WHERE K > 9) AS Z ON T.K = Z.K WHEN MATCHED THEN DELETE";
    assert_eq!(a.run(filtered).unwrap(), Completion::NoData, "a derived table of no row");
    assert_eq!(target(&a), BEFORE);
    let unchanged = "MERGE INTO T USING S ON T.K = S.K WHEN MATCHED AND S.V < 0 THEN DELETE";
    assert_eq!(a.run(unchanged).unwrap(), Completion::Success, "rows, none changed");
    assert_eq!(statement(&a, "ROW_COUNT"), "2", "rows 2 and 3 matched");
    assert_eq!(target(&a), BEFORE);
    let unmatched = "MERGE INTO T USING S ON T.K = S.K + 100 WHEN MATCHED THEN DELETE";
    assert_eq!(a.run(unmatched).unwrap(), Completion::Success, "rows, none matched");
    assert_eq!(statement(&a, "ROW_COUNT"), "0", "no pair matched");
    assert_eq!(target(&a), BEFORE);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 GR2c, GR2f ii, Table 39] after a
/// MERGE, COMMAND_FUNCTION is MERGE and ROW_COUNT, as printed, the rows of
/// the target joined to the source before it, whatever its WHEN
/// conditions: of WHEN MATCHED clauses alone the pairs ON matches \[1\], of
/// WHEN NOT MATCHED clauses alone the source rows it matches to none \[2\],
/// of both the rows of the RIGHT OUTER JOIN \[3\].
#[test]
fn diagnostics_after_merge_23_1() {
    let db = merged();
    let a = db.session("A");
    a.run(UPSERT).unwrap();
    assert_eq!((statement(&a, "COMMAND_FUNCTION"), statement(&a, "ROW_COUNT")), ("MERGE".to_owned(), "3".to_owned()));
    a.rollback().unwrap();
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN DELETE").unwrap();
    assert_eq!(statement(&a, "ROW_COUNT"), "2", "rows 2 and 3 matched");
    a.rollback().unwrap();
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED AND S.V > 1000 THEN INSERT VALUES (S.K, S.V, S.N)").unwrap();
    assert_eq!(statement(&a, "ROW_COUNT"), "1", "row 4 matched to none, though not inserted");
    a.rollback().unwrap();
    let some = "MERGE INTO T USING S ON T.K = S.K \
                WHEN MATCHED AND S.V > 250 THEN DELETE \
                WHEN NOT MATCHED AND S.V > 1000 THEN INSERT VALUES (S.K, S.V, S.N)";
    a.run(some).unwrap();
    assert_eq!(statement(&a, "ROW_COUNT"), "3", "each source row once, though only row 3 is deleted");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12, §20.7; §20.13] RULES.md D45 a
/// MERGE prepared once runs with each set of values given, its dynamic
/// parameters in the ON condition, a WHEN condition, a SET clause and the
/// insert values, each typed by where it stands; one that matches and
/// inserts nothing, of a source of rows, succeeds, ROW_COUNT its source's
/// rows \[ISO/IEC 9075-2:2023 §23.1 GR2f ii 3\].
#[test]
fn a_prepared_merge_runs_with_each_value_given_14_12() {
    let db = merged();
    let a = db.session("A");
    let p = a
        .prepare(
            "MERGE INTO T USING S ON T.K = S.K + ? \
             WHEN MATCHED AND S.V > ? THEN UPDATE SET V = ? \
             WHEN NOT MATCHED AND S.V > ? THEN INSERT (K, V) VALUES (S.K + ?, ?)",
        )
        .unwrap();
    let run = |values: [i32; 6]| a.execute_prepared(&p, &values.map(number)).unwrap();
    assert_eq!(run([0, 250, 7, 0, 0, 8]), Completion::Success);
    assert_eq!(target(&a), ["1,10,a", "2,20,b", "3,7,c", "4,8,none"]);
    assert_eq!(run([10, 0, 5, 1000, 10, 9]), Completion::Success, "nothing matched, nothing inserted");
    assert_eq!(statement(&a, "ROW_COUNT"), "3");
    assert_eq!(run([-1, 0, 5, 0, 10, 9]), Completion::Success);
    assert_eq!(target(&a), ["1,5,a", "2,5,b", "3,5,c", "4,8,none"], "each of rows 1 to 3 matched by its successor");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.2] MERGE and MATCHED, which
/// SQL-92 does not reserve, still name tables and columns, also inside a
/// MERGE: a target named MERGE with a correlation name given without AS,
/// a clause condition reading a column named MATCHED; and a query given
/// directly may begin with a table named MERGE.
#[test]
fn merge_and_matched_are_still_identifiers_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE TABLE MERGE (MATCHED INTEGER, MERGE INTEGER)").unwrap();
    a.run("INSERT INTO MERGE VALUES (1, 2), (3, 2)").unwrap();
    assert_eq!(rows(&a, "SELECT MATCHED, MERGE FROM MERGE ORDER BY MATCHED").unwrap(), ["1,2", "3,2"]);
    let odd = "MERGE INTO MERGE M USING S ON M.MATCHED = S.K \
               WHEN MATCHED AND MATCHED > 2 THEN DELETE \
               WHEN NOT MATCHED THEN INSERT (MATCHED, MERGE) VALUES (S.K, 0)";
    assert_eq!(a.run(odd).unwrap(), Completion::Success);
    assert_eq!(rows(&a, "SELECT MATCHED, MERGE FROM MERGE ORDER BY MATCHED").unwrap(), ["1,2", "2,0", "4,0"]);
    assert!(matches!(a.direct("MERGE CROSS JOIN S").unwrap(), Direct::Rows(_)), "a query");
    a.rollback().unwrap();
}
