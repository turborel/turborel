//! INSTEAD OF triggers that take a MERGE, a positioned UPDATE or DELETE,
//! or a change of a view over their own [ISO/IEC 9075-2:2023 §14.12 GR6a,
//! §15.6 GR11b, §15.7 GR17b, §15.10, §15.12, §15.13, §15.15, §15.16]
//! (Feature T213), and the views WITH CHECK OPTION that may not read
//! their views [§11.32 SR19, §11.49 SR15d].

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use super::merge::statement;

/// Schema A: T (K, N), with rows (1, 10), (1, 5) and (2, 20); V, the
/// grouped, read-only view of K and SUM of N; L, a log in the order its
/// rows came, by SEQ.
fn grouped() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL, N INTEGER)
           CREATE VIEW V (K, N) AS SELECT K, SUM(N) FROM T GROUP BY K
           CREATE TABLE L (SEQ INTEGER GENERATED ALWAYS AS IDENTITY, WHAT VARCHAR(10), K INTEGER, N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (1, 5), (2, 20)").unwrap();
    a.commit().unwrap();
    db
}

/// Schema A: T (K, N, M), with rows (1, 10, 100) and (2, 20, 200); V, the
/// updatable view of all of T; W, the updatable view of N and K of V, in
/// that order; L, a log in the order its rows came.
fn stacked() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, N INTEGER, M INTEGER)
           CREATE VIEW V AS SELECT K, N, M FROM T
           CREATE VIEW W AS SELECT N, K FROM V
           CREATE TABLE L (SEQ INTEGER GENERATED ALWAYS AS IDENTITY, WHAT VARCHAR(10), K INTEGER, N INTEGER, M INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10, 100), (2, 20, 200)").unwrap();
    a.commit().unwrap();
    db
}

/// Defines in `a` an INSTEAD OF trigger of each event on `view`, of
/// columns K and N, each logging its rows to L.
fn logging(a: &Session, view: &str) {
    a.run(&format!(
        "CREATE TRIGGER U_{view} INSTEAD OF UPDATE ON {view} REFERENCING OLD AS O NEW AS NW FOR EACH ROW
           INSERT INTO L (WHAT, K, N) VALUES ('UPD', O.K, NW.N)"
    ))
    .unwrap();
    a.run(&format!(
        "CREATE TRIGGER D_{view} INSTEAD OF DELETE ON {view} REFERENCING OLD AS O FOR EACH ROW INSERT INTO L (WHAT, K, N) VALUES ('DEL', O.K, O.N)"
    ))
    .unwrap();
    a.run(&format!(
        "CREATE TRIGGER I_{view} INSTEAD OF INSERT ON {view} REFERENCING NEW AS NW FOR EACH ROW INSERT INTO L (WHAT, K, N) VALUES ('INS', NW.K, NW.N)"
    ))
    .unwrap();
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// L's rows, in the order they came, without SEQ.
fn logged(s: &Session) -> Vec<String> {
    rows(s, "SELECT WHAT, K, N FROM L ORDER BY SEQ")
}

/// A MERGE into V deleting the rows of N 0, updating the others it
/// matches and inserting what it matches not.
const MERGE_V: &str = "MERGE INTO V USING (VALUES (1, 100), (2, 0), (3, 30)) AS S (K, N) ON V.K = S.K
     WHEN MATCHED AND S.N = 0 THEN DELETE
     WHEN MATCHED THEN UPDATE SET N = V.N + S.N
     WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.N)";

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR2 to SR4, GR6a,
/// §15.10 GR3a, §15.13 GR3a, §15.16 GR3a] a MERGE into a read-only view
/// whose INSTEAD OF triggers take every clause gives each clause's rows,
/// read from the view, to the trigger of its event, clause by clause in
/// order, and writes nothing through the view; ROW_COUNT counts every
/// joined row [§23.1 GR2f ii 3].
#[test]
fn a_merge_into_a_read_only_view_runs_its_triggers() {
    let db = grouped();
    let a = db.session("A");
    logging(&a, "V");
    assert_eq!(a.run(MERGE_V).unwrap(), Completion::Success);
    assert_eq!(statement(&a, "ROW_COUNT"), "3");
    assert_eq!(logged(&a), ["DEL,2,20", "UPD,1,115", "INS,3,30"]);
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K, N"), ["1,5", "1,10", "2,20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR2, SR3] a MERGE into
/// a read-only view is refused, SQLCODE -103, while a clause has no
/// trigger to take it; [GR6a i 1 B II] a target row two source rows match
/// is a cardinality violation, the view's rows read as a table's are.
#[test]
fn a_merge_into_a_read_only_view_needs_a_trigger_for_each_clause() {
    let db = grouped();
    let a = db.session("A");
    a.run("CREATE TRIGGER U_V INSTEAD OF UPDATE ON V REFERENCING NEW AS NW FOR EACH ROW INSERT INTO L (WHAT, K) VALUES ('UPD', NW.K)")
        .unwrap();
    let e = a.run(MERGE_V).unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "{e}");
    let twice = "MERGE INTO V USING (VALUES (1), (1)) AS S (K) ON V.K = S.K WHEN MATCHED THEN UPDATE SET N = 0";
    assert_eq!(a.run(twice).unwrap_err().code, Code::Cardinality);
    assert_eq!(logged(&a), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 5 C II, GR6a
/// ii 8 A, §15.16 GR3a] a MERGE into an updatable view whose INSTEAD OF
/// trigger takes its updates gives the trigger the view's rows before
/// and after the SET clause, and inserts the rows it matches not through
/// the view, as no trigger takes those.
#[test]
fn a_merge_into_an_updatable_view_writes_what_no_trigger_takes() {
    let db = stacked();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER U_V INSTEAD OF UPDATE ON V REFERENCING OLD AS O NEW AS NW FOR EACH ROW
           INSERT INTO L (WHAT, K, N, M) VALUES ('UPD', O.K, NW.N, NW.M)",
    )
    .unwrap();
    let merge = "MERGE INTO V USING (VALUES (1, 11), (3, 33)) AS S (K, N) ON V.K = S.K
         WHEN MATCHED THEN UPDATE SET N = S.N
         WHEN NOT MATCHED THEN INSERT (K, N) VALUES (S.K, S.N)";
    a.run(merge).unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K, N, M FROM L"), ["UPD,1,11,100"]);
    assert_eq!(rows(&a, "SELECT K, N, M FROM T ORDER BY K"), ["1,10,100", "2,20,200", "3,33,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.12 GR2b, §15.13 GR3a,
/// §15.15 GR3c iv 2, §15.16 GR3a] an INSERT, UPDATE, DELETE or MERGE of an
/// updatable view over a view with INSTEAD OF triggers is taken by those
/// triggers, each given rows of their own view, a column the view named
/// lacks null of an insert and as the base table has it otherwise; no row
/// is written.
#[test]
fn a_view_over_a_trigger_view_is_changed_by_its_triggers() {
    let db = stacked();
    let a = db.session("A");
    let log = |what: &str, row: &str| format!("INSERT INTO L (WHAT, K, N, M) VALUES ('{what}', {row}.K, {row}.N, {row}.M)");
    a.run(&format!("CREATE TRIGGER I_V INSTEAD OF INSERT ON V REFERENCING NEW AS NW FOR EACH ROW {}", log("INS", "NW"))).unwrap();
    a.run(&format!("CREATE TRIGGER U_V INSTEAD OF UPDATE ON V REFERENCING NEW AS NW FOR EACH ROW {}", log("UPD", "NW"))).unwrap();
    a.run(&format!("CREATE TRIGGER D_V INSTEAD OF DELETE ON V REFERENCING OLD AS O FOR EACH ROW {}", log("DEL", "O"))).unwrap();
    a.run("INSERT INTO W VALUES (30, 3)").unwrap();
    a.run("UPDATE W SET N = N + 1 WHERE K = 1").unwrap();
    a.run("DELETE FROM W WHERE K = 2").unwrap();
    a.run("MERGE INTO W USING (VALUES (1)) AS S (K) ON W.K = S.K WHEN MATCHED THEN DELETE").unwrap();
    let all = rows(&a, "SELECT WHAT, K, N, M FROM L ORDER BY SEQ");
    assert_eq!(all, ["INS,3,30,NULL", "UPD,1,11,100", "DEL,2,20,200", "DEL,1,10,100"]);
    assert_eq!(rows(&a, "SELECT K, N, M FROM T ORDER BY K"), ["1,10,100", "2,20,200"]);
}

/// The type of a fetched K.
fn integer() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.6 GR11b, GR13, §15.7
/// GR16, GR17b] a positioned UPDATE or DELETE through a cursor over a view
/// with INSTEAD OF triggers gives the row the cursor is on, the view's
/// whole row, to the trigger of its event, and writes nothing; after the
/// DELETE the cursor is before the next row.
#[test]
fn a_positioned_change_of_a_view_runs_its_triggers() {
    let db = stacked();
    let a = db.session("A");
    logging(&a, "W");
    a.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM W").unwrap();
    let mut p = Params::new().declare("K_OUT", integer(), Value::Null);
    a.execute("OPEN C", &mut p).unwrap();
    a.execute("FETCH C INTO K_OUT", &mut p).unwrap();
    a.execute("UPDATE W SET N = 0 WHERE CURRENT OF C", &mut p).unwrap();
    a.execute("DELETE FROM W WHERE CURRENT OF C", &mut p).unwrap();
    a.execute("FETCH C INTO K_OUT", &mut p).unwrap();
    assert_eq!(p.value("K_OUT").to_string(), "2");
    assert_eq!(logged(&a), ["UPD,1,0", "DEL,1,10"]);
    assert_eq!(rows(&a, "SELECT K, N, M FROM T ORDER BY K"), ["1,10,100", "2,20,200"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 AR1b iii, GR6a ii 7,
/// §15.13 GR3a] an insert a trigger takes in a MERGE needs INSERT on each
/// column it names of the view, and gives the trigger a row of each
/// column named its value, every other null, DEFAULT too.
#[test]
fn a_merge_insert_a_trigger_takes_is_a_row_of_the_view() {
    let db = grouped();
    let a = db.session("A");
    logging(&a, "V");
    let merge = "MERGE INTO V USING (VALUES (7)) AS S (K) ON V.K = S.K WHEN NOT MATCHED THEN INSERT (K) VALUES (S.K)";
    a.run(merge).unwrap();
    let defaulted = "MERGE INTO V USING (VALUES (8)) AS S (K) ON V.K = S.K WHEN NOT MATCHED THEN INSERT VALUES (S.K, DEFAULT)";
    a.run(defaulted).unwrap();
    assert_eq!(logged(&a), ["INS,7,NULL", "INS,8,NULL"]);
    a.run("GRANT SELECT ON V TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    let e = b.run("MERGE INTO A.V USING (VALUES (9)) AS S (K) ON A.V.K = S.K WHEN NOT MATCHED THEN INSERT (K) VALUES (S.K)").unwrap_err();
    assert_eq!(e.code, Code::Privilege, "{e}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.10 GR3a, §15.21] a
/// prepared MERGE into a view its triggers take reads the view's rows as
/// they are at each execution, and a statement trigger of a clause
/// executes once with its transition table.
#[test]
fn a_prepared_merge_reads_the_view_each_time() {
    let db = grouped();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER D_V INSTEAD OF DELETE ON V REFERENCING OLD TABLE AS GONE INSERT INTO L (WHAT, N) SELECT 'DEL', COUNT(*) FROM GONE",
    )
    .unwrap();
    let merge = "MERGE INTO V USING (VALUES (1), (2), (3)) AS S (K) ON V.K = S.K WHEN MATCHED THEN DELETE";
    let p = a.prepare(merge).unwrap();
    a.execute_prepared(&p, &[]).unwrap();
    a.run("INSERT INTO T VALUES (3, 1)").unwrap();
    a.execute_prepared(&p, &[]).unwrap();
    assert_eq!(logged(&a), ["DEL,NULL,2", "DEL,NULL,3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.32 SR19, §11.49 SR15d] a
/// view WITH CHECK OPTION reads no view with an INSTEAD OF trigger, down
/// the views it reads, so neither is defined over the other.
#[test]
fn a_view_with_check_option_reads_no_trigger_view() {
    let db = stacked();
    let a = db.session("A");
    a.run("CREATE TRIGGER D_V INSTEAD OF DELETE ON V DELETE FROM L").unwrap();
    let e = a.run("CREATE VIEW X AS SELECT N, K FROM W WHERE K > 0 WITH CHECK OPTION").unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "{e}");
    a.run("DROP TRIGGER D_V").unwrap();
    a.run("CREATE VIEW X AS SELECT N, K FROM W WHERE K > 0 WITH CHECK OPTION").unwrap();
    let e = a.run("CREATE TRIGGER D_V INSTEAD OF DELETE ON V DELETE FROM L").unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "{e}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR13, §15.10, §15.13,
/// §15.16, §6.6 SR9] a trigger's action may change a read-only view whose
/// INSTEAD OF triggers take the change, by INSERT, UPDATE, DELETE or
/// MERGE, its WHERE clause reading the action's own new row or new table
/// beside the view's rows.
#[test]
fn a_triggered_action_changes_a_trigger_view() {
    let db = grouped();
    let a = db.session("A");
    logging(&a, "V");
    a.run("CREATE TABLE Q (K INTEGER)").unwrap();
    let after = |name: &str, action: &str| {
        a.run(&format!("CREATE TRIGGER {name} AFTER INSERT ON Q REFERENCING NEW AS NW FOR EACH ROW {action}")).unwrap();
    };
    after("QI", "INSERT INTO V VALUES (NW.K, 1)");
    after("QU", "UPDATE V SET N = 0 WHERE K = NW.K - 8");
    a.run("CREATE TRIGGER QD AFTER INSERT ON Q REFERENCING NEW TABLE AS ADDED DELETE FROM V WHERE K IN (SELECT K - 7 FROM ADDED)").unwrap();
    after("QM", "MERGE INTO V USING (VALUES (NW.K)) AS S (K) ON V.K = S.K - 8 WHEN MATCHED THEN DELETE");
    a.run("INSERT INTO Q VALUES (9)").unwrap();
    assert_eq!(logged(&a), ["INS,9,1", "UPD,1,0", "DEL,2,20", "DEL,1,15"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.5, §15.16 GR3a] the new
/// row an INSTEAD OF trigger is given of an update is the old with its SET
/// clauses assigned, a generated column of the base table as it was, as
/// no base table is written, by a MERGE and a positioned UPDATE alike.
#[test]
fn a_trigger_view_row_keeps_its_generated_values() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, N INTEGER, G INTEGER GENERATED ALWAYS AS (N * 2))
           CREATE VIEW V AS SELECT K, N, G FROM T
           CREATE TABLE L (K INTEGER, N INTEGER, G INTEGER)
           CREATE TRIGGER U_V INSTEAD OF UPDATE ON V REFERENCING NEW AS NW FOR EACH ROW INSERT INTO L VALUES (NW.K, NW.N, NW.G)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (K, N) VALUES (1, 10)").unwrap();
    a.run("MERGE INTO V USING (VALUES (1)) AS S (K) ON V.K = S.K WHEN MATCHED THEN UPDATE SET N = 5").unwrap();
    a.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM V").unwrap();
    let mut p = Params::new().declare("K_OUT", integer(), Value::Null);
    a.execute("OPEN C", &mut p).unwrap();
    a.execute("FETCH C INTO K_OUT", &mut p).unwrap();
    a.execute("UPDATE V SET N = 6 WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(rows(&a, "SELECT K, N, G FROM L ORDER BY N"), ["1,5,20", "1,6,20"]);
}
