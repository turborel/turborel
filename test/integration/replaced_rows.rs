//! \[DESIGN\] The row an UPDATE or DELETE replaced, as the write found it,
//! not read before it is written (RULES.md D417): what a trigger's old
//! row, the undo of a statement and a transaction's rollback see of it.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

use crate::common::sorted;

/// Schema A: T of the row store, and C of the column store frozen: K, a
/// key, V a number, B a long text kept out of line, of C a long character
/// string; and O, each old row an update or delete replaced, as its
/// trigger saw it.
fn defined(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER PRIMARY KEY, V INTEGER, B CHARACTER LARGE OBJECT(100K))
           CREATE TABLE C (K INTEGER PRIMARY KEY, V INTEGER, B VARCHAR(30000)) USING columnstore
           CREATE TABLE O (T VARCHAR(1), K INTEGER, V INTEGER, L INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for t in ["T", "C"] {
        for (when, old) in [("UPDATE", "OLD"), ("DELETE", "OLD")] {
            a.run(&format!(
                "CREATE TRIGGER {t}_{when} AFTER {when} ON {t} REFERENCING {old} ROW AS X FOR EACH ROW
                   INSERT INTO O VALUES ('{t}', X.K, X.V, CHARACTER_LENGTH(X.B))"
            ))
            .unwrap();
        }
        for k in 1..=4_usize {
            a.run(&format!("INSERT INTO {t} VALUES ({k}, {}, '{}')", k * 10, "x".repeat(9000 + k))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    a
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.13, §15.7]; [DESIGN] RULES.md
/// D417: the old rows of an update of a row committed, of one already
/// changed in the transaction, of a delete after an update, each with its
/// long text out of line, are the rows each write replaced, in the row
/// store and of rows frozen in the column store; a statement refused
/// leaves the rows as they were, and a rollback restores them.
#[test]
fn each_write_replaces_the_row_it_found() {
    let db = Database::new();
    let a = defined(&db);
    for t in ["T", "C"] {
        a.run(&format!("UPDATE {t} SET V = V + 1, B = '{}' WHERE K = 1", "y".repeat(20000))).unwrap();
        a.run(&format!("UPDATE {t} SET V = V + 1 WHERE K = 1")).unwrap();
        a.run(&format!("UPDATE {t} SET V = V + 5 WHERE K = 2")).unwrap();
        a.run(&format!("DELETE FROM {t} WHERE K = 2")).unwrap();
        a.run(&format!("DELETE FROM {t} WHERE K = 3")).unwrap();
        a.run(&format!("UPDATE {t} SET K = 1 WHERE K = 4")).unwrap_err();
    }
    let olds = ["1,10,9001", "1,11,20000", "2,20,9002", "2,25,9002", "3,30,9003"];
    for t in ["T", "C"] {
        let mut want: Vec<String> = olds.iter().map(ToString::to_string).collect();
        want.sort();
        assert_eq!(sorted(&a, &format!("SELECT K, V, L FROM O WHERE T = '{t}'")), want, "{t}");
        assert_eq!(sorted(&a, &format!("SELECT K, V, CHARACTER_LENGTH(B) FROM {t}")), vec!["1,12,20000", "4,40,9004"], "{t}");
    }
    a.rollback().unwrap();
    for t in ["T", "C"] {
        let all = vec!["1,10,9001", "2,20,9002", "3,30,9003", "4,40,9004"];
        assert_eq!(sorted(&a, &format!("SELECT K, V, CHARACTER_LENGTH(B) FROM {t}")), all, "{t}");
    }
}
