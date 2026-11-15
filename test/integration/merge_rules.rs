//! MERGE's Syntax Rules and Access Rules [ISO/IEC 9075-2:2023 §14.12],
//! each refusal with its exact SQLCODE; a view as the target, WITH CHECK
//! OPTION or not; identity and generated columns of the target; and the
//! READ ONLY transaction [GR1].

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlError};

use crate::common::{failure, rows};
use crate::merge::{BEFORE, UPSERT, merged, state, target};

/// The SQLCODE class of a refusal, none if `result` succeeded.
fn refused<T>(result: Result<T, SqlError>) -> Option<Code> {
    result.err().map(|e| e.code)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR8, SR9, SR11, SR12; §6.6] the
/// source is no joined table, but may be a derived table over one; it
/// exposes no name the target exposes, a table name or a correlation
/// name, but the target may be read as the source under another; the
/// target's name is not in scope in the insert values, where an
/// unqualified column is the source's; a table or column that does not
/// exist is refused. Each refusal SQLCODE -103.
#[test]
fn the_source_and_its_scope_refused_by_their_syntax_rules_14_12() {
    let db = merged();
    let a = db.session("A");
    for (sql, rule) in [
        ("MERGE INTO T USING S CROSS JOIN E ON T.K = S.K WHEN MATCHED THEN DELETE", "SR11"),
        ("MERGE INTO T USING S JOIN E ON S.K = E.K ON T.K = S.K WHEN MATCHED THEN DELETE", "SR11, a qualified join"),
        ("MERGE INTO T USING T ON T.K = T.K WHEN MATCHED THEN DELETE", "SR12, the table name"),
        ("MERGE INTO T AS X USING S AS X ON X.K = X.K WHEN MATCHED THEN DELETE", "SR12, both correlation names"),
        ("MERGE INTO T AS S USING S ON S.K = S.K WHEN MATCHED THEN DELETE", "SR12, the target's correlation name"),
        ("MERGE INTO T USING S AS T ON T.K = T.K WHEN MATCHED THEN DELETE", "SR12, the source's correlation name"),
        ("MERGE INTO NONE USING S ON NONE.K = S.K WHEN MATCHED THEN DELETE", "no target"),
        ("MERGE INTO T USING NONE ON T.K = NONE.K WHEN MATCHED THEN DELETE", "no source"),
        ("MERGE INTO T USING S ON T.K = S.X WHEN MATCHED THEN DELETE", "no column X"),
        ("MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN INSERT VALUES (T.K, S.V, S.N)", "SR9, T"),
        ("MERGE INTO T USING (SELECT K, V FROM S) AS Z ON T.K = Z.K WHEN NOT MATCHED THEN INSERT VALUES (Z.K, Z.V, N)", "SR9, T's N"),
    ] {
        assert_eq!(refused(a.run(sql)), Some(Code::SyntaxRule), "{rule}: {sql}");
    }
    assert_eq!(target(&a), BEFORE);
    let joined = "MERGE INTO T USING (SELECT S.K FROM S CROSS JOIN E) AS J ON T.K = J.K WHEN MATCHED THEN DELETE";
    assert_eq!(a.run(joined).unwrap(), Completion::NoData, "a derived table, of no row");
    a.run("MERGE INTO T AS X USING T ON X.K = T.K + 1 WHEN MATCHED THEN UPDATE SET V = T.V").unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,10,b", "3,20,c"], "T as the source, X the target");
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN INSERT VALUES (K, V, N)").unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,10,b", "3,20,c", "4,400,D"], "S's columns");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR17, SR18a, SR18d, SR18g; §14.15
/// SR2, SR8; §9.2 SR] an insert column list names each column of T once;
/// the values are as many as its columns, or as T's columns without one;
/// each value assignable to its column; a SET clause sets a column of T,
/// once. Each refusal SQLCODE -103; the MERGE's format refusals -102: no
/// WHEN clause, INSERT where a WHEN MATCHED clause has UPDATE or DELETE,
/// and those where a WHEN NOT MATCHED clause has INSERT, INSERT of no
/// VALUES or of more than one row, two tables after USING.
#[test]
fn the_clauses_refused_by_their_rules_14_12() {
    let db = merged();
    let a = db.session("A");
    let on = "MERGE INTO T USING S ON T.K = S.K";
    for (clause, rule) in [
        ("WHEN NOT MATCHED THEN INSERT (K, K) VALUES (S.K, S.K)", "SR17"),
        ("WHEN NOT MATCHED THEN INSERT (K, V) VALUES (S.K)", "SR18d, too few"),
        ("WHEN NOT MATCHED THEN INSERT (K, V) VALUES (S.K, S.V, S.N)", "SR18d, too many"),
        ("WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V)", "SR18a, SR18d"),
        ("WHEN NOT MATCHED THEN INSERT (K, X) VALUES (S.K, 1)", "no column X"),
        ("WHEN NOT MATCHED THEN INSERT (K, V) VALUES (S.K, S.N)", "SR18g"),
        ("WHEN MATCHED THEN UPDATE SET V = 1, V = 2", "§14.15 SR8"),
        ("WHEN MATCHED THEN UPDATE SET X = 1", "§14.15 SR2"),
        ("WHEN MATCHED THEN UPDATE SET V = S.N", "§9.2 SR"),
    ] {
        assert_eq!(refused(a.run(&format!("{on} {clause}"))), Some(Code::SyntaxRule), "{rule}: {clause}");
    }
    for clause in [
        "",
        "WHEN MATCHED THEN INSERT VALUES (S.K, S.V, S.N)",
        "WHEN NOT MATCHED THEN UPDATE SET V = 1",
        "WHEN NOT MATCHED THEN DELETE",
        "WHEN NOT MATCHED THEN INSERT DEFAULT VALUES",
        "WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V, S.N), (5, 5, 'x')",
        "WHEN NOT MATCHED THEN INSERT SELECT * FROM S",
    ] {
        assert_eq!(refused(a.run(&format!("{on} {clause}"))), Some(Code::Syntax), "{clause}");
    }
    let two = "MERGE INTO T USING S, E ON T.K = S.K WHEN MATCHED THEN DELETE";
    assert_eq!(refused(a.run(two)), Some(Code::Syntax), "two tables");
    assert_eq!(target(&a), BEFORE);
}

/// The privileges P is given on T and S, `merging` needing each.
const GRANTS: [&str; 4] =
    ["GRANT SELECT ON S TO P", "GRANT UPDATE (V) ON T TO P", "GRANT DELETE ON T TO P", "GRANT INSERT (K, V) ON T TO P"];

/// A MERGE of every kind of clause, as P runs it.
const MERGING: &str = "MERGE INTO A.T AS X USING A.S AS Y ON X.K = Y.K \
     WHEN MATCHED AND Y.V > 250 THEN DELETE \
     WHEN MATCHED THEN UPDATE SET V = Y.V \
     WHEN NOT MATCHED THEN INSERT (K, V) VALUES (Y.K, Y.V)";

/// `merged`, P given SELECT on T and each of `GRANTS` but the one at
/// `left_out`, if any.
fn granted(left_out: Option<usize>) -> Database {
    let db = merged();
    let a = db.session("A");
    a.run("GRANT SELECT ON T TO P").unwrap();
    for (i, grant) in GRANTS.iter().enumerate() {
        if left_out != Some(i) {
            a.run(grant).unwrap();
        }
    }
    a.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 AR1b i, ii, iii; §7.6 AR] a MERGE
/// needs UPDATE on each update object column, DELETE on T, INSERT on each
/// insert object column, the implicit list's too, and SELECT on the
/// source. With all of them it runs, and without any one it is refused
/// (SQLCODE -104), as it is setting or inserting a column not granted.
#[test]
fn each_privilege_needed_14_12() {
    let db = granted(None);
    let p = db.session("P");
    assert_eq!(p.run(MERGING).unwrap(), Completion::Success);
    p.commit().unwrap();
    assert_eq!(target(&db.session("A")), ["1,10,a", "2,200,b", "4,400,none"]);
    let set = "MERGE INTO A.T AS X USING A.S AS Y ON X.K = Y.K WHEN MATCHED THEN UPDATE SET N = Y.N";
    assert_eq!(refused(p.run(set)), Some(Code::Privilege), "AR1b i, N");
    let implicit = "MERGE INTO A.T AS X USING A.S AS Y ON X.K = Y.K WHEN NOT MATCHED THEN INSERT VALUES (Y.K, Y.V, Y.N)";
    assert_eq!(refused(p.run(implicit)), Some(Code::Privilege), "AR1b iii, SR18a: N");
    for (i, grant) in GRANTS.iter().enumerate() {
        let db = granted(Some(i));
        assert_eq!(refused(db.session("P").run(MERGING)), Some(Code::Privilege), "without {grant}");
        assert_eq!(target(&db.session("A")), BEFORE, "without {grant}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR2, SR3, SR4, SR7, GR6a i 5 C II,
/// GR6a i 7 B II, GR6a ii 8 B] a MERGE into an updatable view changes its
/// base table: its rows updated, deleted, inserted, one outside the view
/// inserted too without CHECK OPTION; a row of the base table outside the
/// view is matched by none, so its source row is unmatched, and inserting
/// it duplicates the key (SQLSTATE 23000); a grouped view, neither
/// updatable nor insertable-into, is refused (SQLCODE -103).
#[test]
fn a_view_merged_into_changes_its_base_table_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE VIEW W AS SELECT K, V FROM T WHERE V < 100").unwrap();
    a.run("CREATE VIEW R AS SELECT V, COUNT(*) AS C FROM T GROUP BY V").unwrap();
    a.run("INSERT INTO T VALUES (5, 500, 'e')").unwrap();
    a.run("INSERT INTO S VALUES (5, 1, 'E')").unwrap();
    a.commit().unwrap();
    a.run("MERGE INTO W USING S ON W.K = S.K WHEN MATCHED THEN UPDATE SET V = S.V / 100").unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,2,b", "3,3,c", "5,500,e"], "row 5 not W's");
    a.rollback().unwrap();
    let mixed = "MERGE INTO W USING S ON W.K = S.K WHEN MATCHED THEN DELETE WHEN NOT MATCHED AND S.K = 4 THEN INSERT VALUES (S.K, S.V)";
    a.run(mixed).unwrap();
    assert_eq!(target(&a), ["1,10,a", "4,400,none", "5,500,e"]);
    a.rollback().unwrap();
    let hidden = "MERGE INTO W USING S ON W.K = S.K WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V)";
    assert_eq!(failure(a.run(hidden)), (Code::Constraint, "23000".to_owned()), "row 5 of S unmatched");
    for clause in ["WHEN MATCHED THEN DELETE", "WHEN MATCHED THEN UPDATE SET C = 1", "WHEN NOT MATCHED THEN INSERT VALUES (S.V, 1)"] {
        let sql = format!("MERGE INTO R USING S ON R.V = S.V {clause}");
        assert_eq!(refused(a.run(&sql)), Some(Code::SyntaxRule), "{clause}");
    }
    assert_eq!(target(&a), ["1,10,a", "2,20,b", "3,30,c", "5,500,e"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR7; §15.17] a view WITH CHECK
/// OPTION refuses a row a MERGE updates or inserts outside it, false or
/// unknown of its condition (SQLCODE -201, SQLSTATE 44000), the statement
/// undone; rows within it are written, and deleted rows are not checked.
#[test]
fn check_option_refuses_rows_outside_the_view_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE VIEW C AS SELECT K, V FROM T WHERE V < 100 WITH CHECK OPTION").unwrap();
    a.commit().unwrap();
    for clause in [
        "WHEN MATCHED THEN UPDATE SET V = S.V",
        "WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V)",
        "WHEN NOT MATCHED THEN INSERT VALUES (S.K, NULL)",
        "WHEN MATCHED THEN UPDATE SET V = 1 WHEN NOT MATCHED THEN INSERT VALUES (S.K, 100)",
    ] {
        let sql = format!("MERGE INTO C USING S ON C.K = S.K {clause}");
        assert_eq!(failure(a.run(&sql)), (Code::Constraint, "44000".to_owned()), "{clause}");
        assert_eq!(target(&a), BEFORE, "{clause}");
    }
    let within =
        "MERGE INTO C USING S ON C.K = S.K WHEN MATCHED THEN UPDATE SET V = S.V / 100 WHEN NOT MATCHED THEN INSERT VALUES (S.K, 4)";
    a.run(within).unwrap();
    assert_eq!(target(&a), ["1,10,a", "2,2,b", "3,3,c", "4,4,none"]);
    a.run("MERGE INTO C USING S ON C.K = S.K WHEN MATCHED THEN DELETE").unwrap();
    assert_eq!(target(&a), ["1,10,a"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR1, GR8] in a READ ONLY
/// transaction a MERGE into a persistent table is refused (SQLSTATE
/// 25006), of an empty source too, GR1 coming before GR8; one into a
/// temporary table runs, though its source is persistent. MC/DC of GR1's
/// two conditions: read-only and a persistent target refused; read-write
/// and the same target run (every other test); read-only and a temporary
/// target run.
#[test]
fn a_read_only_transaction_merges_into_temporary_tables_alone_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE GLOBAL TEMPORARY TABLE G (K INTEGER, V INTEGER) ON COMMIT PRESERVE ROWS").unwrap();
    a.commit().unwrap();
    a.run("SET TRANSACTION READ ONLY").unwrap();
    assert_eq!(state(&a, UPSERT), "25006");
    assert_eq!(state(&a, "MERGE INTO T USING E ON T.K = E.K WHEN MATCHED THEN DELETE"), "25006", "an empty source");
    let temporary = "MERGE INTO G USING S ON G.K = S.K WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V)";
    assert_eq!(a.run(temporary).unwrap(), Completion::Success);
    assert_eq!(rows(&a, "SELECT K, V FROM G ORDER BY K").unwrap(), ["2,200", "3,300", "4,400"]);
    a.rollback().unwrap();
    assert_eq!(target(&a), BEFORE);
}

/// The least and greatest K of the rows of `table` whose V is 200, 300 or
/// 400 plus `offset`, the identity values one MERGE gave.
fn drawn(s: &Session, table: &str, offset: i32) -> String {
    let values = [200, 300, 400].map(|v| (v + offset).to_string()).join(", ");
    rows(s, &format!("SELECT MIN(K), MAX(K) FROM {table} WHERE V IN ({values})")).unwrap().concat()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR18b, SR18h ii, iii, GR6a ii 7 D
/// IV, V] an identity column GENERATED ALWAYS given a value needs an
/// `<override clause>`. OVERRIDING SYSTEM VALUE keeps the value, OVERRIDING
/// USER VALUE draws one; left out or DEFAULT it draws; an override clause
/// with no identity column given a value is refused, as is OVERRIDING
/// SYSTEM VALUE for one GENERATED BY DEFAULT, which takes a value given
/// (SQLCODE -103).
#[test]
fn identity_columns_and_the_override_clause_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE TABLE GA (K INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 100), V INTEGER)").unwrap();
    a.run("CREATE TABLE GD (K INTEGER GENERATED BY DEFAULT AS IDENTITY (START WITH 100), V INTEGER)").unwrap();
    let into = |table: &str, insert: &str| format!("MERGE INTO {table} USING S ON 1 = 0 WHEN NOT MATCHED THEN {insert}");
    for (sql, rule) in [
        (into("GA", "INSERT VALUES (S.K, S.V)"), "SR18h ii"),
        (into("GA", "INSERT (V) OVERRIDING SYSTEM VALUE VALUES (S.V)"), "SR18h, none given"),
        (into("GD", "INSERT OVERRIDING SYSTEM VALUE VALUES (S.K, S.V)"), "SR18h iii"),
    ] {
        assert_eq!(refused(a.run(&sql)), Some(Code::SyntaxRule), "{rule}");
    }
    a.run(&into("GA", "INSERT (V) VALUES (S.V)")).unwrap();
    a.run(&into("GA", "INSERT VALUES (DEFAULT, S.V + 1)")).unwrap();
    a.run(&into("GA", "INSERT OVERRIDING SYSTEM VALUE VALUES (S.K, S.V + 2)")).unwrap();
    a.run(&into("GA", "INSERT OVERRIDING USER VALUE VALUES (S.K, S.V + 3)")).unwrap();
    let ga: Vec<String> = (0..4).map(|o| drawn(&a, "GA", o)).collect();
    assert_eq!(ga, ["100,102", "103,105", "2,4", "106,108"], "drawn, drawn, kept, drawn");
    a.run(&into("GD", "INSERT VALUES (S.K, S.V)")).unwrap();
    a.run(&into("GD", "INSERT OVERRIDING USER VALUE VALUES (S.K, S.V + 1)")).unwrap();
    assert_eq!([drawn(&a, "GD", 0), drawn(&a, "GD", 1)], ["2,4", "100,102"], "kept, drawn");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR18b, SR18f; §14.15 SR5; §15.11
/// GR6a; §15.14 GR9] a generated column is not given a value by a MERGE,
/// in its insert values nor its SET clause (SQLCODE -103), but DEFAULT; it
/// takes its expression's value of each row inserted and each row updated.
#[test]
fn generated_columns_computed_on_insert_and_update_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE TABLE G (A INTEGER, B INTEGER, C INTEGER GENERATED ALWAYS AS (A + B))").unwrap();
    a.run("INSERT INTO G (A, B) VALUES (2, 1)").unwrap();
    let on = "MERGE INTO G USING S ON G.A = S.K";
    for clause in [
        "WHEN NOT MATCHED THEN INSERT VALUES (S.K, 1, 5)",
        "WHEN NOT MATCHED THEN INSERT (A, C) VALUES (S.K, 5)",
        "WHEN MATCHED THEN UPDATE SET C = 5",
    ] {
        assert_eq!(refused(a.run(&format!("{on} {clause}"))), Some(Code::SyntaxRule), "{clause}");
    }
    a.run(&format!("{on} WHEN MATCHED THEN UPDATE SET B = S.V WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V, DEFAULT)")).unwrap();
    assert_eq!(rows(&a, "SELECT A, B, C FROM G ORDER BY A").unwrap(), ["2,200,202", "3,300,303", "4,400,404"]);
    a.run("MERGE INTO G USING S ON G.A = S.K + 10 WHEN NOT MATCHED THEN INSERT (B, A) VALUES (1, S.K + 10)").unwrap();
    assert_eq!(rows(&a, "SELECT A, C FROM G WHERE A > 10 ORDER BY A").unwrap(), ["12,13", "13,14", "14,15"]);
    a.rollback().unwrap();
}
