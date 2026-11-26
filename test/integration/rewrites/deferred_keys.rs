//! A key whose constraint is deferrable is no key to the rules that
//! group by a key alone or fetch by one (RULES.md D1201, D1202): it may
//! be broken until it is checked [ISO/IEC 9075-2:2023 §4.25.2], and two
//! rows of one value in it are then two groups.

use turborel::db::{Database, Session};
use turborel::optimizer::{FETCH_DEPENDENTS_LATE, REDUCE_GROUPING_KEYS};

use super::{COLUMNS, alike, left, tables};

/// Schema Q's tables, and of each store three tables of 12 parents, K
/// NOT NULL, a name and a number: IP, K its PRIMARY KEY; DP, K its
/// PRIMARY KEY by a constraint DEFERRABLE INITIALLY DEFERRED; DU, K
/// UNIQUE by such a constraint; and DC of 60 children, each of a parent.
/// Their statistics are gathered. Then, in the transaction left open, DP
/// and DU each take a second row of K = 3, of another name and number.
fn with_deferred() -> (Database, Session) {
    let (db, s) = tables();
    for (prefix, store) in [("", ""), (COLUMNS, " USING columnstore")] {
        let parent = "K INTEGER NOT NULL, N VARCHAR(10), A INTEGER";
        for table in [
            format!("IP ({parent}, PRIMARY KEY (K))"),
            format!("DP ({parent}, CONSTRAINT {prefix}DPK PRIMARY KEY (K) DEFERRABLE INITIALLY DEFERRED)"),
            format!("DU ({parent}, CONSTRAINT {prefix}DUK UNIQUE (K) DEFERRABLE INITIALLY DEFERRED)"),
            "DC (K INTEGER NOT NULL PRIMARY KEY, P INTEGER, V INTEGER NOT NULL)".to_owned(),
        ] {
            s.run(&format!("CREATE TABLE {prefix}{table}{store}")).unwrap();
        }
        let parents: Vec<String> = (0..12).map(|k| format!("({k}, 'n{}', {})", k % 4, k % 3)).collect();
        for table in ["IP", "DP", "DU"] {
            s.run(&format!("INSERT INTO {prefix}{table} VALUES {}", parents.join(", "))).unwrap();
        }
        let children: Vec<String> = (0..60).map(|k| format!("({k}, {}, {})", k % 12, k % 7)).collect();
        s.run(&format!("INSERT INTO {prefix}DC VALUES {}", children.join(", "))).unwrap();
    }
    s.commit().unwrap();
    for table in ["IP", "DP", "DU", "DC"] {
        db.analyze(&format!("Q.{table}")).unwrap();
        db.analyze(&format!("Q.{COLUMNS}{table}")).unwrap();
    }
    db.maintain().unwrap();
    for prefix in ["", COLUMNS] {
        for table in ["DP", "DU"] {
            s.run(&format!("INSERT INTO {prefix}{table} VALUES (3, 'twin', 9)")).unwrap();
        }
    }
    (db, s)
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.2, §4.26.3]; [DESIGN] RULES.md
/// D1201: a grouping by a key and the columns it would determine, the
/// key a PRIMARY KEY or UNIQUE NOT NULL by a constraint DEFERRABLE
/// INITIALLY DEFERRED and two rows of one value of it in the open
/// transaction, is left grouped by all its columns, of either store, of
/// one table and of a join, and gives the two rows two groups, the rows
/// it gives with the rule off; of the same table with its key immediate
/// the rule groups by the key.
#[test]
fn a_deferred_key_broken_groups_by_all_its_columns() {
    let (_db, s) = with_deferred();
    let one = "SELECT P.K, P.N, P.A, COUNT(*) FROM {T} P GROUP BY P.K, P.N, P.A";
    let joined = "SELECT P.K, P.N, SUM(C.V), COUNT(*) FROM {T} P, {DC} C WHERE C.P = P.K GROUP BY P.K, P.N";
    for sql in [one, joined] {
        alike(&s, REDUCE_GROUPING_KEYS, &sql.replace("{T}", "{IP}"));
        for table in ["{DP}", "{DU}"] {
            for found in left(&s, REDUCE_GROUPING_KEYS, &sql.replace("{T}", table)) {
                assert_eq!(found.matches("\"3,").count(), 2, "{table}: two groups of K = 3\n{found}");
                assert_eq!(found.matches("\"3,twin,").count(), 1, "{table}\n{found}");
            }
        }
    }
    s.rollback().unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.2, §7.17 GR7]; [DESIGN] RULES.md
/// D1202: a query keeping the first rows of its groups, a column carried
/// of a table whose key is by a constraint DEFERRABLE INITIALLY DEFERRED
/// and holds two rows of one value in the open transaction, is left, the
/// column carried and never fetched by that key, of either store, and
/// gives each of the two rows its group, the rows it gives with the rule
/// off; of the same table with its key immediate the column is fetched
/// late.
#[test]
fn a_deferred_key_broken_fetches_nothing_late() {
    let (_db, s) = with_deferred();
    let sql = "SELECT P.K, P.N, P.A, SUM(C.V) FROM {T} P, {DC} C WHERE C.P = P.K GROUP BY P.K, P.N, P.A \
               ORDER BY P.K, P.A FETCH FIRST 5 ROWS ONLY";
    alike(&s, FETCH_DEPENDENTS_LATE, &sql.replace("{T}", "{IP}"));
    for table in ["{DP}", "{DU}"] {
        for found in left(&s, FETCH_DEPENDENTS_LATE, &sql.replace("{T}", table)) {
            let keys: Vec<usize> = ["\"0,", "\"1,", "\"2,", "\"3,", "\"4,"].iter().map(|k| found.matches(k).count()).collect();
            assert_eq!(keys, [1, 1, 1, 2, 0], "{table}: the first five groups\n{found}");
            assert!(found.contains("\"3,twin,9,"), "{table}\n{found}");
        }
    }
    s.rollback().unwrap();
}
