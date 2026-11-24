//! \[EXTENSION\] EXPLAIN of a data change statement without ANALYZE
//! (RULES.md D980 to D984): INSERT, UPDATE, DELETE and MERGE, searched
//! and positioned, shown as EXPLAIN ANALYZE shows them without what ran,
//! with the indexes each maintains, the triggers that would fire, the
//! referential actions that would cascade and the point path that would
//! run it; the files, the rows and the locks of the database as they
//! were; and the privileges preparing the statement needs.

use turborel::db::{Database, Direct, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{Scratch, bytes_of, exec, locking_database, rows};

/// The tables of these cases, of ANN: P, keyed; C, whose rows reference
/// P's, deleted with them and set to null as their keys change, indexed
/// on W; Q, keyed, with no trigger or reference; LOG, which each trigger
/// writes; and PV, a view of P whose INSERT a trigger takes.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION ANN
  CREATE TABLE P (K INTEGER PRIMARY KEY, V INTEGER)
  CREATE TABLE C (K INTEGER PRIMARY KEY, P INTEGER REFERENCES P ON DELETE CASCADE ON UPDATE SET NULL, W INTEGER)
  CREATE TABLE Q (K INTEGER PRIMARY KEY, V INTEGER)
  CREATE TABLE LOG (E VARCHAR(20))
  CREATE VIEW PV AS SELECT K, V FROM P";

/// The rows and triggers of `SCHEMA`'s tables.
const FILLED: [&str; 6] = [
    "INSERT INTO P VALUES (1, 1), (2, 2), (3, 3), (4, 4)",
    "INSERT INTO C VALUES (1, 1, 10), (2, 2, 20), (3, 3, 30), (5, 4, 50)",
    "INSERT INTO Q VALUES (1, 1), (2, 2)",
    "CREATE TRIGGER P_SET AFTER UPDATE OF V ON P FOR EACH ROW INSERT INTO LOG VALUES ('p set')",
    "CREATE TRIGGER C_GONE AFTER DELETE ON C FOR EACH STATEMENT INSERT INTO LOG VALUES ('c gone')",
    "CREATE TRIGGER PV_ADD INSTEAD OF INSERT ON PV FOR EACH ROW INSERT INTO LOG VALUES ('pv')",
];

/// A data change of each kind, each of whose EXPLAIN changes nothing.
const CHANGES: [&str; 9] = [
    "INSERT INTO P VALUES (100, 1)",
    "INSERT INTO Q SELECT K + 100, W FROM C WHERE W > 30",
    "UPDATE P SET V = 5 WHERE K = 3",
    "UPDATE C SET W = 5 WHERE K = 3",
    "UPDATE P SET K = K + 100 WHERE V > 1",
    "DELETE FROM P WHERE K = 4",
    "DELETE FROM C",
    "MERGE INTO P USING C ON P.K = C.K WHEN MATCHED THEN UPDATE SET V = C.W WHEN NOT MATCHED THEN INSERT VALUES (C.K, C.W)",
    "INSERT INTO PV VALUES (7, 7)",
];

/// Queries of every row of `SCHEMA`'s tables, in order.
const ALL_ROWS: [&str; 4] =
    ["SELECT K, V FROM P ORDER BY K", "SELECT K, P, W FROM C ORDER BY K", "SELECT K, V FROM Q ORDER BY K", "SELECT E FROM LOG ORDER BY E"];

/// `db` given `SCHEMA` and `FILLED`, committed, and C's index on W.
fn filled(db: Database) -> Database {
    db.schema(SCHEMA).unwrap();
    let s = db.session("ANN");
    for sql in FILLED {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    db.create_index("C_W", "ANN.C", &["W"], "btree").unwrap();
    db
}

/// The rows of each of `ALL_ROWS`, as `s` reads them.
fn all_rows(s: &Session) -> Vec<Vec<String>> {
    ALL_ROWS.iter().map(|sql| rows(s, sql).unwrap()).collect()
}

/// The lines of the rows `sql`, a direct statement, gives, or its error.
fn direct_lines(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let Direct::Rows(mut cursor) = s.direct(sql).map_err(|e| e.code)? else { panic!("{sql} gives rows") };
    let mut lines = Vec::new();
    while let Some(row) = cursor.fetch().map_err(|e| e.code)? {
        lines.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(lines)
}

/// The text `Session::explain` gives of `sql`.
fn shown(s: &Session, sql: &str) -> String {
    s.explain(sql, &Params::new()).unwrap()
}

/// Verifies: [EXTENSION] RULES.md D980, D983: EXPLAIN of an INSERT of
/// values names its table, the index it maintains and its values
/// evaluated with no plan; of an UPDATE by a key, the probe of the key
/// below it, no index maintained of a column no key reads, the trigger
/// of its column, and the point read a prepared statement or a procedure
/// makes; of an UPDATE of an indexed column, that index; of an UPDATE of
/// a referenced key, the action that sets the references to null; of a
/// DELETE, the action that deletes the rows referencing it and the
/// statement-level trigger of their table.
#[test]
fn each_searched_change_shows_its_plan_and_what_it_would_do() {
    let db = filled(Database::new());
    let s = db.session("ANN");
    assert_eq!(
        shown(&s, CHANGES[0]),
        "-- physical:\nInsert into ANN.P\n-- maintains: ANN.P.UNIQUE1 [btree]\n-- point path: its values evaluated, with no plan\n"
    );
    let by_key = "-- physical:\nUpdate ANN.P\n  IndexScan ANN.P using ANN.P.UNIQUE1 [btree]: P.K = 3  (est. rows=1, cost=14.6)\n\
                  -- maintains: none\n-- trigger: ANN.P_SET, AFTER UPDATE OF V FOR EACH ROW on ANN.P\n\
                  -- point path: one read of ANN.P.UNIQUE1 when prepared or in a procedure\n";
    assert_eq!(shown(&s, CHANGES[2]), by_key);
    assert!(shown(&s, CHANGES[3]).contains("\n-- maintains: C_W [btree]\n"), "{}", shown(&s, CHANGES[3]));
    let rekeyed = shown(&s, CHANGES[4]);
    assert!(rekeyed.contains("  Filter P.V > 1  (est. rows=1)\n    SeqScan ANN.P"), "{rekeyed}");
    assert!(
        rekeyed.contains("\n-- maintains: ANN.P.UNIQUE1 [btree]\n-- cascades: ON UPDATE SET NULL to ANN.C by ANN.C_FOREIGN1\n"),
        "{rekeyed}"
    );
    assert!(rekeyed.ends_with("\n-- point path: none\n"), "{rekeyed}");
    let deleted = shown(&s, CHANGES[5]);
    assert!(deleted.starts_with("-- physical:\nDelete from ANN.P\n  IndexScan ANN.P"), "{deleted}");
    let cascaded =
        "\n-- cascades: ON DELETE CASCADE to ANN.C by ANN.C_FOREIGN1\n-- trigger: ANN.C_GONE, AFTER DELETE FOR EACH STATEMENT on ANN.C\n";
    assert!(deleted.contains(cascaded), "{deleted}");
}

/// Verifies: [EXTENSION] RULES.md D980: EXPLAIN of a MERGE shows the plan
/// of its joined rows sorted by its target's rows, the indexes its
/// clauses maintain and the triggers its UPDATE activates; of an INSERT
/// into a view an INSTEAD OF trigger takes, the trigger alone; of an
/// INSERT of a query's rows, the query's plan. As a direct statement its
/// rows are the lines of the library's text.
#[test]
fn a_merge_an_insert_of_a_query_and_a_view_its_trigger_takes_are_shown() {
    let db = filled(Database::new());
    let s = db.session("ANN");
    let merged = shown(&s, CHANGES[7]);
    assert!(merged.starts_with("-- physical:\nMerge into ANN.P\n  Sort row id\n"), "{merged}");
    assert!(merged.contains("LeftOuterJoin P.K = C.K"), "{merged}");
    let effects =
        "\n-- maintains: ANN.P.UNIQUE1 [btree]\n-- trigger: ANN.P_SET, AFTER UPDATE OF V FOR EACH ROW on ANN.P\n-- point path: none\n";
    assert!(merged.ends_with(effects), "{merged}");
    assert_eq!(
        shown(&s, CHANGES[8]),
        "-- physical:\nInsert into ANN.PV\n-- trigger: ANN.PV_ADD, INSTEAD OF INSERT FOR EACH ROW on ANN.PV\n-- point path: none\n"
    );
    let inserted = shown(&s, CHANGES[1]);
    assert!(inserted.starts_with("-- physical:\nInsert into ANN.Q\n  "), "{inserted}");
    assert!(inserted.contains("ANN.C"), "{inserted}");
    for sql in CHANGES {
        let text: Vec<String> = shown(&s, sql).lines().map(str::to_owned).collect();
        assert_eq!(direct_lines(&s, &format!("EXPLAIN {sql}")), Ok(text), "{sql}");
    }
}

/// Verifies: [EXTENSION] RULES.md D984, D841, D752: a change of a table
/// no trigger or reference reaches runs straight in a procedure once its
/// probe is a point read, and an INSERT of a query's rows does too, so
/// EXPLAIN says so; EXECUTOR.COMPILED_PROCEDURES OFF takes the procedure's
/// path away, EXECUTOR.POINT_OPERATIONS OFF the point read; a statement
/// of a procedure's parameters, given by the library, is planned with
/// their values.
#[test]
fn the_point_path_is_the_one_a_prepared_statement_or_procedure_runs() {
    let db = filled(Database::new());
    let s = db.session("ANN");
    let point = |sql: &str| shown(&s, sql).lines().last().unwrap().to_owned();
    let both = "-- point path: one read of ANN.Q.UNIQUE1 when prepared or in a procedure, its row written straight in a procedure";
    assert_eq!(point("UPDATE Q SET V = 0 WHERE K = 2"), both);
    assert_eq!(point("DELETE FROM Q WHERE K = 2"), both);
    assert_eq!(point(CHANGES[1]), "-- point path: its rows written straight in a procedure");
    assert_eq!(
        point("INSERT INTO Q VALUES (9, 9)"),
        "-- point path: its values evaluated, with no plan, its rows written straight in a procedure"
    );
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };
    let params = Params::new().declare("KK", integer.clone(), Value::Exact(Decimal::new(2, 0))).declare(
        "NV",
        integer,
        Value::Exact(Decimal::new(0, 0)),
    );
    let text = s.explain("UPDATE Q SET V = NV WHERE K = KK", &params).unwrap();
    assert!(text.contains("IndexScan ANN.Q using ANN.Q.UNIQUE1") && text.ends_with(&format!("{both}\n")), "{text}");
    s.run("SET EXECUTOR.COMPILED_PROCEDURES = 'OFF'").unwrap();
    assert_eq!(point("UPDATE Q SET V = 0 WHERE K = 2"), "-- point path: one read of ANN.Q.UNIQUE1 when prepared or in a procedure");
    s.run("SET EXECUTOR.POINT_OPERATIONS = 'OFF'").unwrap();
    assert_eq!(point("UPDATE Q SET V = 0 WHERE K = 2"), "-- point path: none");
}

/// Verifies: [EXTENSION] RULES.md D980, [X3.135-1986 §8.11 SR2, §8.4
/// SR2]: a positioned UPDATE or DELETE, by the library, shows its
/// cursor's row in place of a plan, its cursor declared, open or not; of
/// a cursor not declared it is refused, as direct SQL, which declares
/// none, refuses it.
#[test]
fn a_positioned_change_shows_its_cursors_row() {
    let db = filled(Database::new());
    let s = db.session("ANN");
    let sql = "UPDATE P SET V = 1 WHERE CURRENT OF CP";
    assert_eq!(s.explain(sql, &Params::new()).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(direct_lines(&s, &format!("EXPLAIN {sql}")), Err(Code::SyntaxRule));
    s.declare_cursor("DECLARE CP CURSOR FOR SELECT K, V FROM P FOR UPDATE OF V").unwrap();
    let expected = "-- physical:\nUpdate ANN.P\n  CurrentRow of cursor CP\n-- maintains: none\n\
                    -- trigger: ANN.P_SET, AFTER UPDATE OF V FOR EACH ROW on ANN.P\n-- point path: none\n";
    assert_eq!(shown(&s, sql), expected);
    let deleted = shown(&s, "DELETE FROM P WHERE CURRENT OF CP");
    assert!(deleted.starts_with("-- physical:\nDelete from ANN.P\n  CurrentRow of cursor CP\n"), "{deleted}");
    assert!(deleted.contains("-- cascades: ON DELETE CASCADE to ANN.C by ANN.C_FOREIGN1"), "{deleted}");
    assert_eq!(s.explain("DELETE FROM P WHERE CURRENT OF NONE", &Params::new()).unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] RULES.md D982: EXPLAIN of each kind of change
/// leaves the database's files byte for byte as they were, its tables'
/// rows and indexes and its log alike, and every row as it was; no
/// trigger fired, LOG holding no row.
#[test]
fn explain_of_each_change_leaves_the_files_as_they_were() {
    let scratch = Scratch::new("explain-changes");
    let dir = scratch.path("db");
    let db = filled(Database::open(&dir).unwrap());
    db.checkpoint().unwrap();
    let s = db.session("ANN");
    let before = (bytes_of(&dir, "data"), bytes_of(&dir, "log"), all_rows(&s));
    s.rollback().unwrap();
    s.declare_cursor("DECLARE CP CURSOR FOR SELECT K, V FROM P FOR UPDATE OF V").unwrap();
    for sql in CHANGES {
        assert!(direct_lines(&s, &format!("EXPLAIN {sql}")).is_ok(), "{sql}");
    }
    shown(&s, "UPDATE P SET V = 1 WHERE CURRENT OF CP");
    shown(&s, "DELETE FROM P WHERE CURRENT OF CP");
    s.rollback().unwrap();
    let after = (bytes_of(&dir, "data"), bytes_of(&dir, "log"), all_rows(&s));
    assert!(!before.0.is_empty() && !before.1.is_empty(), "a data file and a log are read");
    assert!(before.0 == after.0, "the data file and its indexes as they were");
    assert!(before.1 == after.1, "the log as it was");
    assert_eq!(before.2, after.2);
    exec(&s, "UPDATE Q SET V = 9 WHERE K = 1").unwrap();
    s.commit().unwrap();
    assert!(bytes_of(&dir, "log") != after.1, "a change run is logged");
    drop(s);
    db.close().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D982: no trigger fires for EXPLAIN of a
/// change that activates one, row-level, statement-level or INSTEAD OF,
/// in the session's transaction or after it commits; each change run
/// fires its trigger, as the lines of EXPLAIN said.
#[test]
fn no_trigger_fires() {
    let db = filled(Database::new());
    let s = db.session("ANN");
    for sql in [CHANGES[2], CHANGES[6], CHANGES[8]] {
        direct_lines(&s, &format!("EXPLAIN {sql}")).unwrap();
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM LOG").unwrap(), ["0"], "{sql}");
    }
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM LOG").unwrap(), ["0"]);
    for sql in [CHANGES[2], CHANGES[6], CHANGES[8]] {
        exec(&s, sql).unwrap();
    }
    assert_eq!(rows(&s, "SELECT E FROM LOG ORDER BY E").unwrap(), ["c gone", "p set", "pv"]);
}

/// Verifies: [EXTENSION] RULES.md D982: EXPLAIN locks no row and no
/// table, of the row store and of the locking engine alike: while the
/// transaction that explained each change is open, a second session
/// changes the rows they would change and commits, waiting for none. The
/// change itself, run, locks them, so the second session is refused.
#[test]
fn no_row_is_locked_from_a_second_session() {
    for db in [filled(Database::new()), filled(locking_database())] {
        let (one, two) = (db.session("ANN"), db.session("ANN"));
        for sql in ["UPDATE P SET V = 5 WHERE K = 3", "DELETE FROM P WHERE K = 3", CHANGES[7], "INSERT INTO P VALUES (3, 3)"] {
            direct_lines(&one, &format!("EXPLAIN {sql}")).unwrap();
        }
        exec(&two, "UPDATE P SET V = 30 WHERE K = 3").unwrap();
        exec(&two, "DELETE FROM C WHERE K = 1").unwrap();
        two.commit().unwrap();
        one.commit().unwrap();
        assert_eq!(rows(&one, "SELECT V FROM P WHERE K = 3").unwrap(), ["30"]);
        exec(&one, "UPDATE P SET V = 5 WHERE K = 3").unwrap();
        assert!(exec(&two, "UPDATE P SET V = 31 WHERE K = 3").is_err(), "the row the change wrote is locked");
        two.rollback().unwrap();
        one.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] RULES.md D981, [ISO/IEC 9075-2:2023 §20.7 GR6]:
/// EXPLAIN of a change needs the privileges preparing it needs, those of
/// its access rules: refused as preparing it is refused, by the same
/// code, without them; shown with them, a column's UPDATE privilege for
/// the column it sets alone; and in a READ ONLY transaction, which the
/// change itself may not run in.
#[test]
fn explain_needs_the_privileges_preparing_needs() {
    let db = filled(Database::new());
    let ann = db.session("ANN");
    ann.run("GRANT SELECT ON P TO BOB").unwrap();
    ann.commit().unwrap();
    let bob = db.session("BOB");
    let refused = |sql: &str| (bob.explain(sql, &Params::new()).unwrap_err().code, bob.prepare(sql).unwrap_err().code);
    for sql in ["UPDATE ANN.P SET V = 1 WHERE K = 1", "DELETE FROM ANN.P", "INSERT INTO ANN.P VALUES (9, 9)"] {
        assert_eq!(refused(sql), (Code::Privilege, Code::Privilege), "{sql}");
        assert_eq!(direct_lines(&bob, &format!("EXPLAIN {sql}")), Err(Code::Privilege), "{sql}");
    }
    ann.run("GRANT UPDATE (V) ON P TO BOB").unwrap();
    ann.commit().unwrap();
    bob.rollback().unwrap();
    assert!(bob.explain("UPDATE ANN.P SET V = 1 WHERE K = 1", &Params::new()).unwrap().contains("Update ANN.P"));
    assert_eq!(refused("UPDATE ANN.P SET K = 1"), (Code::Privilege, Code::Privilege));
    bob.rollback().unwrap();
    bob.run("SET TRANSACTION READ ONLY").unwrap();
    assert!(direct_lines(&bob, "EXPLAIN UPDATE ANN.P SET V = 1 WHERE K = 1").is_ok());
    assert!(exec(&bob, "UPDATE ANN.P SET V = 1 WHERE K = 1").is_err(), "the change itself is not of a READ ONLY transaction");
}

/// Verifies: [EXTENSION] RULES.md D980: EXPLAIN of a change of a table
/// not defined is refused as binding it is, SQLCODE -103, by the
/// library and as a direct statement.
#[test]
fn a_change_of_no_table_is_refused() {
    let db = filled(Database::new());
    let s = db.session("ANN");
    assert_eq!(s.explain("UPDATE NOPE SET V = 1", &Params::new()).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(direct_lines(&s, "EXPLAIN DELETE FROM NOPE"), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] RULES.md D983, D875: an UPDATE of a column no
/// index key reads maintains no index, but where a BEFORE row trigger
/// may set any column of the row it writes, every index of the table.
#[test]
fn a_before_row_trigger_makes_an_update_maintain_every_index() {
    let db = filled(Database::new());
    let s = db.session("ANN");
    s.run("CREATE TABLE B (K INTEGER PRIMARY KEY, W INTEGER, X INTEGER)").unwrap();
    s.commit().unwrap();
    db.create_index("B_W", "ANN.B", &["W"], "btree").unwrap();
    let maintains = |s: &Session| shown(s, "UPDATE B SET X = 1").lines().find(|l| l.starts_with("-- maintains:")).unwrap().to_owned();
    assert_eq!(maintains(&s), "-- maintains: none");
    s.run("CREATE TRIGGER B_FIX BEFORE UPDATE ON B REFERENCING NEW ROW AS N FOR EACH ROW SET N.W = N.X").unwrap();
    assert_eq!(maintains(&s), "-- maintains: ANN.B.UNIQUE1 [btree], B_W [btree]");
}
