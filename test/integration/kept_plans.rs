//! Plans kept between executions of a prepared statement (RULES.md D45): a
//! query, an INSERT and a searched or positioned UPDATE or DELETE bound
//! once, each execution filling in its values, in a session with declared
//! local temporary tables too; bound again when the definitions, the
//! settings or the session it was bound with changed, or at each execution
//! when its plan holds the statement's instant; its results always those
//! of the statement written with its values.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::{Cursor, Database, Prepared, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{Rng, exec, rows, sorted};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Every row of `cursor`, as `rows` gives a query's.
fn all(mut cursor: Cursor) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(r) = cursor.fetch().unwrap() {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// The rows of `p`, a query, opened with `values`.
fn opened(s: &Session, p: &Prepared, values: &[Value]) -> Vec<String> {
    all(s.open_prepared(p, values).unwrap())
}

/// Schema X with T (K INTEGER NOT NULL, N INTEGER, S VARCHAR(8)) of rows
/// K 1 to 20, N K mod 5, S 'S' and K.
fn schema() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, N INTEGER, S VARCHAR(8))").unwrap();
    let s = db.session("X");
    for k in 1..=20 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {}, 'S{k}')", k % 5)).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR5, §17.10, §17.14];
/// [DESIGN] RULES.md D45: a prepared query, searched UPDATE and DELETE,
/// and INSERT, executed many times with other values, make one plan each,
/// and give what the statement written with the values gives.
#[test]
fn a_prepared_statement_keeps_its_plan_between_executions() {
    let (_db, s) = schema();
    let query = s.prepare("SELECT K, S FROM T WHERE K = ? OR N = ? ORDER BY K").unwrap();
    for k in 1..=8 {
        let direct = rows(&s, &format!("SELECT K, S FROM T WHERE K = {k} OR N = {} ORDER BY K", k % 3)).unwrap();
        assert_eq!(opened(&s, &query, &[int(k), int(k % 3)]), direct, "K = {k}");
    }
    assert_eq!(query.plans_made(), 1);
    let update = s.prepare("UPDATE T SET N = N + ? WHERE K = ?").unwrap();
    let delete = s.prepare("DELETE FROM T WHERE K = ?").unwrap();
    let insert = s.prepare("INSERT INTO T (K, S) VALUES (?, ?)").unwrap();
    for k in 1..=4 {
        s.execute_prepared(&update, &[int(100), int(k)]).unwrap();
        s.execute_prepared(&delete, &[int(k + 10)]).unwrap();
        s.execute_prepared(&insert, &[int(k + 30), Value::Char(format!("I{k}"))]).unwrap();
    }
    assert_eq!([update.plans_made(), delete.plans_made(), insert.plans_made()], [1, 1, 1]);
    let changed = rows(&s, "SELECT K, N, S FROM T WHERE K < 5 OR K BETWEEN 11 AND 14 OR K > 30 ORDER BY K").unwrap();
    assert_eq!(changed, ["1,101,S1", "2,102,S2", "3,103,S3", "4,104,S4", "31,NULL,I1", "32,NULL,I2", "33,NULL,I3", "34,NULL,I4"]);
    // A copy shares what its original keeps.
    let copy = query.clone();
    assert_eq!(opened(&s, &copy, &[int(2), int(9)]), ["2,S2"]);
    assert_eq!(copy.plans_made(), 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR5f]; [DESIGN] RULES.md
/// D45: a value its parameter's type takes none of fails as CAST fails,
/// SQLSTATE 22018, whether the plan is kept or made, and the plan stays
/// kept.
#[test]
fn a_value_that_suits_no_parameter_fails_as_cast_does() {
    let (_db, s) = schema();
    let sql = "SELECT K FROM T WHERE K = ?";
    let fails = |p: &Prepared| s.open_prepared(p, &[Value::Char("ABC".into())]).err().map(|e| (e.code, e.sqlstate().as_str().to_owned()));
    let kept = s.prepare(sql).unwrap();
    assert_eq!(opened(&s, &kept, &[int(1)]), ["1"]);
    let made = s.prepare(sql).unwrap();
    assert_eq!(fails(&kept), fails(&made));
    assert_eq!(fails(&kept).unwrap().1, "22018");
    assert_eq!(opened(&s, &kept, &[int(2)]), ["2"]);
    assert_eq!(kept.plans_made(), 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR10]; [DESIGN] RULES.md
/// D45: a kept plan is made again after an index is made or dropped, a
/// column added, a setting of the database or of the session set, and
/// gives the rows of the definitions of then.
#[test]
fn a_kept_plan_is_made_again_when_what_it_was_made_with_changes() {
    let (db, s) = schema();
    let query = s.prepare("SELECT * FROM T WHERE K = ?").unwrap();
    assert_eq!(opened(&s, &query, &[int(2)]), ["2,2,S2"]);
    db.create_index("T_K", "X.T", &["K"], "btree").unwrap();
    assert_eq!(opened(&s, &query, &[int(3)]), ["3,3,S3"]);
    assert_eq!(query.plans_made(), 2, "an index made");
    s.run("ALTER TABLE T ADD COLUMN M INTEGER DEFAULT 7").unwrap();
    s.commit().unwrap();
    assert_eq!(opened(&s, &query, &[int(4)]), ["4,4,S4,7"], "the column added is read");
    s.commit().unwrap();
    s.run("DROP INDEX T_K").unwrap();
    assert_eq!(opened(&s, &query, &[int(5)]), ["5,0,S5,7"]);
    assert_eq!(query.plans_made(), 4, "a column added, an index dropped");
    db.set_setting("PLANNER.EQUAL_SELECTIVITY", &Value::Approx(0.5)).unwrap();
    assert_eq!(opened(&s, &query, &[int(6)]), ["6,1,S6,7"]);
    exec(&s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert_eq!(opened(&s, &query, &[int(7)]), ["7,2,S7,7"]);
    assert_eq!(opened(&s, &query, &[int(8)]), ["8,3,S8,7"]);
    assert_eq!(query.plans_made(), 6, "a setting of the database, then of the session");
    s.run("DROP TABLE T CASCADE").unwrap();
    assert_eq!(s.open_prepared(&query, &[int(1)]).err().map(|e| e.code), Some(Code::SyntaxRule), "T is no more");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR10, §10.4 AR1];
/// [DESIGN] RULES.md D45: a privilege revoked is missing at the next
/// execution of a statement whose plan was kept while it was held.
#[test]
fn a_privilege_revoked_is_missing_at_the_next_execution() {
    let (db, x) = schema();
    x.run("GRANT SELECT, UPDATE ON T TO Y").unwrap();
    x.commit().unwrap();
    let y = db.session("Y");
    let query = y.prepare("SELECT K FROM X.T WHERE K = ?").unwrap();
    let update = y.prepare("UPDATE X.T SET N = ? WHERE K = ?").unwrap();
    assert_eq!(opened(&y, &query, &[int(1)]), ["1"]);
    y.execute_prepared(&update, &[int(9), int(1)]).unwrap();
    y.commit().unwrap();
    x.run("REVOKE SELECT, UPDATE ON T FROM Y CASCADE").unwrap();
    x.commit().unwrap();
    assert_eq!(y.open_prepared(&query, &[int(1)]).err().map(|e| e.code), Some(Code::Privilege));
    assert_eq!(y.execute_prepared(&update, &[int(9), int(2)]).err().map(|e| e.code), Some(Code::Privilege));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, §17.6 GR10]; [DESIGN]
/// RULES.md D27, D45: a plan kept is made again on the definitions the
/// session's transaction changed, uncommitted, and again once it rolled
/// back.
#[test]
fn a_kept_plan_follows_the_transactions_own_definitions() {
    let (_db, s) = schema();
    let query = s.prepare("SELECT * FROM T WHERE K = ?").unwrap();
    assert_eq!(opened(&s, &query, &[int(1)]), ["1,1,S1"]);
    s.run("ALTER TABLE T ADD COLUMN M INTEGER DEFAULT 5").unwrap();
    assert_eq!(opened(&s, &query, &[int(1)]), ["1,1,S1,5"], "the transaction's own column");
    s.rollback().unwrap();
    assert_eq!(opened(&s, &query, &[int(1)]), ["1,1,S1"], "rolled back");
    assert_eq!(query.plans_made(), 3);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §16.2, §16.4, §4.30];
/// [DESIGN] RULES.md D45: a plan kept is made again when the session's
/// default schema or authorization identifier changes, so its names and
/// privileges are those of then.
#[test]
fn a_kept_plan_follows_the_sessions_schema_and_user() {
    let (db, x) = schema();
    // The database's owner, who may take another's identifier [§16.4 GR5].
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION Y CREATE TABLE T (K INTEGER NOT NULL, N INTEGER, S VARCHAR(8))").unwrap();
    let y = db.session("Y");
    y.run("INSERT INTO T VALUES (100, 0, 'Y')").unwrap();
    y.run("GRANT SELECT ON T TO X").unwrap();
    y.commit().unwrap();
    let query = x.prepare("SELECT S FROM T WHERE K >= ?").unwrap();
    assert_eq!(opened(&x, &query, &[int(20)]), ["S20"]);
    exec(&x, "SET SCHEMA 'Y'").unwrap();
    assert_eq!(opened(&x, &query, &[int(20)]), ["Y"], "T is Y.T");
    let user = x.prepare("SELECT CURRENT_USER FROM X.T WHERE K = ?").unwrap();
    assert_eq!(opened(&x, &user, &[int(1)]), ["X"]);
    x.commit().unwrap();
    exec(&x, "SET SESSION AUTHORIZATION 'Y'").unwrap();
    assert_eq!(x.open_prepared(&user, &[int(1)]).err().map(|e| e.code), Some(Code::Privilege), "Y may not read X.T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.8 GR3, §11.5 GR2, §13.8
/// GR4b i]; [DESIGN] RULES.md D45: a plan that holds the statement's
/// instant, of CURRENT_TIMESTAMP, of a DEFAULT written or of a view's
/// condition, is made at each execution; an INSERT's columns not written
/// take their defaults anew from a kept plan.
#[test]
fn a_plan_holding_the_instant_is_made_at_each_execution() {
    let (db, s) = schema();
    let clock = Arc::new(FixedClock::at(0));
    db.set_clock(Arc::clone(&clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    s.run("CREATE TABLE E (K INTEGER NOT NULL, W TIMESTAMP(0) WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP)").unwrap();
    s.run("CREATE VIEW V AS SELECT K, W FROM E WHERE W <= CURRENT_TIMESTAMP").unwrap();
    s.commit().unwrap();
    let now = s.prepare("SELECT CURRENT_TIMESTAMP FROM T WHERE K = ?").unwrap();
    let implied = s.prepare("INSERT INTO E (K) VALUES (?)").unwrap();
    let written = s.prepare("INSERT INTO E VALUES (?, DEFAULT)").unwrap();
    let seen = s.prepare("SELECT K FROM V WHERE K >= ? ORDER BY K").unwrap();
    let mut instants = Vec::new();
    for (k, seconds) in [(1, 0), (2, 60), (3, 120)] {
        clock.set(seconds * 1_000_000);
        instants.extend(opened(&s, &now, &[int(1)]));
        s.execute_prepared(&implied, &[int(k)]).unwrap();
        s.execute_prepared(&written, &[int(k + 10)]).unwrap();
        s.commit().unwrap();
        // Rows of this instant and before are seen.
        assert_eq!(opened(&s, &seen, &[int(0)]).len(), 2 * usize::try_from(k).unwrap());
    }
    assert_eq!(instants, ["1970-01-01 00:00:00.000000+00:00", "1970-01-01 00:01:00.000000+00:00", "1970-01-01 00:02:00.000000+00:00"]);
    let w = rows(&s, "SELECT K, W FROM E ORDER BY K").unwrap();
    assert_eq!(w[0..3], ["1,1970-01-01 00:00:00+00:00", "2,1970-01-01 00:01:00+00:00", "3,1970-01-01 00:02:00+00:00"]);
    assert_eq!(w[3..6], ["11,1970-01-01 00:00:00+00:00", "12,1970-01-01 00:01:00+00:00", "13,1970-01-01 00:02:00+00:00"]);
    assert_eq!([now.plans_made(), implied.plans_made(), written.plans_made(), seen.plans_made()], [3, 1, 3, 3]);
}

/// Parameters FK and FN, INTEGER, a FETCH's targets.
fn targets() -> Params {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    Params::new().declare("FK", integer, int(0)).declare("FN", integer, int(0))
}

/// Verifies: [EXTENSION] [X3.135-1986 §8.4, §8.11; ISO/IEC 9075:1992
/// §17.6 GR10, §17.19, §17.20]; [DESIGN] RULES.md D45: a positioned UPDATE
/// and DELETE prepared keep their plan between executions, each changing
/// the row its cursor is on, with its values; made again once an index is
/// made; the privilege it was made with revoked, refused at the next
/// execution. Regression test (R-TEST-7): they were bound at each
/// execution.
#[test]
fn positioned_statements_keep_their_plan_between_executions() {
    let (db, x) = schema();
    x.run("GRANT SELECT, UPDATE, DELETE ON T TO Y").unwrap();
    x.commit().unwrap();
    let y = db.session("Y");
    y.declare_cursor("DECLARE C CURSOR FOR SELECT K, N FROM X.T WHERE K <= 6").unwrap();
    let update = y.prepare("UPDATE X.T SET N = ? WHERE CURRENT OF C").unwrap();
    let delete = y.prepare("DELETE FROM X.T WHERE CURRENT OF C").unwrap();
    let mut p = targets();
    y.execute("OPEN C", &mut p).unwrap();
    for k in 1..=6_i128 {
        y.execute("FETCH C INTO FK, FN", &mut p).unwrap();
        if k % 2 == 1 {
            y.execute_prepared(&update, &[int(k * 10)]).unwrap();
        } else {
            y.execute_prepared(&delete, &[]).unwrap();
        }
    }
    y.execute("CLOSE C", &mut p).unwrap();
    y.commit().unwrap();
    assert_eq!(rows(&y, "SELECT K, N FROM X.T WHERE K <= 6 ORDER BY K").unwrap(), ["1,10", "3,30", "5,50"]);
    assert_eq!([update.plans_made(), delete.plans_made()], [1, 1]);
    db.create_index("T_K", "X.T", &["K"], "btree").unwrap();
    y.execute("OPEN C", &mut p).unwrap();
    y.execute("FETCH C INTO FK, FN", &mut p).unwrap();
    y.execute_prepared(&update, &[int(7)]).unwrap();
    y.execute("CLOSE C", &mut p).unwrap();
    y.commit().unwrap();
    assert_eq!(update.plans_made(), 2, "an index made");
    x.run("REVOKE UPDATE ON T FROM Y CASCADE").unwrap();
    x.commit().unwrap();
    y.execute("OPEN C", &mut p).unwrap();
    y.execute("FETCH C INTO FK, FN", &mut p).unwrap();
    assert_eq!(y.execute_prepared(&update, &[int(8)]).err().map(|e| e.code), Some(Code::Privilege));
    assert_eq!(rows(&y, "SELECT N FROM X.T WHERE K = 1").unwrap(), ["7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.11, §17.6 GR10]; [DESIGN]
/// RULES.md D45: a session that declared local temporary tables keeps its
/// prepared statements' plans, of those tables and of others, while they
/// and the definitions under them are unchanged; a table declared after,
/// an index made, or the session's user changed, makes them again.
/// Regression test (R-TEST-7): such a session kept no plan.
#[test]
fn a_session_with_declared_tables_keeps_its_plans() {
    let (db, s) = schema();
    s.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (K INTEGER NOT NULL, V VARCHAR(8)) ON COMMIT PRESERVE ROWS").unwrap();
    let insert = s.prepare("INSERT INTO MODULE.D VALUES (?, ?)").unwrap();
    let declared = s.prepare("SELECT V FROM MODULE.D WHERE K = ?").unwrap();
    let query = s.prepare("SELECT S FROM T WHERE K = ?").unwrap();
    for k in 1..=4 {
        s.execute_prepared(&insert, &[int(k), Value::Char(format!("D{k}"))]).unwrap();
        assert_eq!(opened(&s, &declared, &[int(k)]), [format!("D{k}")]);
        assert_eq!(opened(&s, &query, &[int(k)]), [format!("S{k}")]);
        s.commit().unwrap();
    }
    assert_eq!([insert.plans_made(), declared.plans_made(), query.plans_made()], [1, 1, 1]);
    s.run("DECLARE LOCAL TEMPORARY TABLE MODULE.E (K INTEGER)").unwrap();
    assert_eq!(opened(&s, &declared, &[int(2)]), ["D2"]);
    assert_eq!(opened(&s, &query, &[int(2)]), ["S2"]);
    assert_eq!([declared.plans_made(), query.plans_made()], [2, 2], "a table declared");
    db.create_index("T_K", "X.T", &["K"], "btree").unwrap();
    assert_eq!(opened(&s, &query, &[int(3)]), ["S3"]);
    assert_eq!(opened(&s, &query, &[int(4)]), ["S4"]);
    assert_eq!(query.plans_made(), 3, "an index made");
    s.commit().unwrap();
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    exec(&s, "SET SESSION AUTHORIZATION 'Y'").unwrap();
    assert_eq!(s.open_prepared(&query, &[int(1)]).err().map(|e| e.code), Some(Code::Privilege), "Y may not read X.T");
    assert_eq!(opened(&s, &declared, &[int(1)]), ["D1"], "its declared tables are the session's, whoever its user");
    assert_eq!(declared.plans_made(), 3, "its user changed");
}

/// A prepared statement of the random test, and how to write it with its
/// values: its text, the parts of it the values go between.
struct Form {
    prepared: Prepared,
    parts: Vec<&'static str>,
}

impl Form {
    fn new(s: &Session, parts: &[&'static str]) -> Form {
        Form { prepared: s.prepare(&parts.join("?")).unwrap(), parts: parts.to_vec() }
    }

    /// Its parameters' values: the first of `values`.
    fn values(&self, values: &[i128]) -> Vec<Value> {
        values.iter().take(self.parts.len() - 1).map(|v| int(*v)).collect()
    }

    /// Its text with `values` in place of its `?`s.
    fn written(&self, values: &[i128]) -> String {
        let mut text = self.parts[0].to_owned();
        for (v, part) in values.iter().zip(&self.parts[1..]) {
            text.push_str(&v.to_string());
            text.push_str(part);
        }
        text
    }
}

/// The queries of the random test, each split at its `?`s.
const QUERIES: [&[&str]; 6] = [
    &["SELECT K, N, S FROM A WHERE K = ", " OR N > ", " ORDER BY K"],
    &["SELECT N, COUNT(*) FROM A WHERE K BETWEEN ", " AND ", " GROUP BY N ORDER BY N"],
    &["SELECT K FROM A WHERE N IN (", ", ", ") AND EXISTS (SELECT * FROM B WHERE B.K = A.K + ", ") ORDER BY K"],
    &["SELECT K, CASE WHEN N < ", " THEN 'LOW' ELSE S END FROM A WHERE K < ", " ORDER BY K"],
    &["SELECT K FROM A WHERE K > (SELECT MIN(K) FROM B WHERE N = ", ") AND N <> ", " ORDER BY K"],
    &["SELECT COUNT(*), SUM(N) FROM A WHERE N - ", " > K / 10"],
];

/// The writes of the random test, of table A; each is made of table B too,
/// written with its values.
const WRITES: [&[&str]; 3] = [
    &["UPDATE A SET N = N + ", " WHERE K = ", ""],
    &["DELETE FROM A WHERE K = ", " AND N < ", ""],
    &["INSERT INTO A VALUES (", ", ", ", 'NEW')"],
];

/// A change of what plans are made with, made between executions.
const CHANGES: [&str; 5] =
    ["CREATE INDEX A_K ON A (K)", "DROP INDEX A_K", "CREATE INDEX A_N ON A (N)", "DROP INDEX A_N", "SET EXECUTOR.BATCHES = 'OFF'"];

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR5]; [DESIGN] RULES.md
/// D45, R-TEST-5: prepared queries and writes executed with random values,
/// between random changes of indexes and settings, give what the same
/// statements written with the values give; the writes leave their table
/// as the written ones leave its twin.
#[test]
fn kept_plans_answer_as_statements_written_with_their_values() {
    let db = Database::new();
    let columns = "K INTEGER NOT NULL, N INTEGER, S VARCHAR(8)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE A ({columns}) CREATE TABLE B ({columns})")).unwrap();
    let s = db.session("Q");
    for k in 0..60 {
        let n = if k % 7 == 0 { "NULL".to_owned() } else { (k % 9).to_string() };
        s.run(&format!("INSERT INTO A VALUES ({k}, {n}, 'S{k}')")).unwrap();
        s.run(&format!("INSERT INTO B VALUES ({k}, {n}, 'S{k}')")).unwrap();
    }
    s.commit().unwrap();
    let queries: Vec<Form> = QUERIES.iter().map(|parts| Form::new(&s, parts)).collect();
    let writes: Vec<Form> = WRITES.iter().map(|parts| Form::new(&s, parts)).collect();
    let mut rng = Rng::new(0x5EED_D045_0000_0001);
    let mut batches = true;
    for step in 0..300 {
        if rng.chance(8) {
            let change = CHANGES[rng.below(CHANGES.len())];
            let toggled = if batches { change.to_owned() } else { change.replace("'OFF'", "'ON'") };
            batches ^= change.starts_with("SET");
            // An index made twice, or dropped while there is none: refused.
            let _ = s.run(&toggled);
            s.commit().unwrap();
        }
        let values: Vec<i128> = (0..3).map(|_| i128::try_from(rng.below(70)).unwrap() - 5).collect();
        if rng.chance(70) {
            let form = &queries[rng.below(queries.len())];
            let direct = rows(&s, &form.written(&values));
            let kept = s.open_prepared(&form.prepared, &form.values(&values)).map(all).map_err(|e| e.code);
            assert_eq!(kept, direct, "step {step}: {}", form.written(&values));
        } else {
            let form = &writes[rng.below(writes.len())];
            let kept = s.execute_prepared(&form.prepared, &form.values(&values)).map_err(|e| e.code);
            let direct = exec(&s, &form.written(&values).replace(" A ", " B ").replace(" A(", " B("));
            assert_eq!(kept.is_ok(), direct.is_ok(), "step {step}: {}", form.written(&values));
        }
        s.commit().unwrap();
    }
    assert_eq!(sorted(&s, "SELECT K, N, S FROM A"), sorted(&s, "SELECT K, N, S FROM B"));
    let made: u64 = queries.iter().chain(&writes).map(|f| f.prepared.plans_made()).sum();
    assert!(made < 150, "plans are kept between changes: {made} made for 300 executions");
}
