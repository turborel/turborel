//! \[DESIGN\] A morsel whose keys are nearly all apart, its rows past its
//! first batch each made a group of their own (RULES.md D1119): the
//! partitions find each row's group and give what the grouping run alone
//! gives.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike, of 30,000 rows: A, one of 25,000 spread so a morsel's
/// rows are nearly all apart in it; B, one of 40 in runs, so a morsel's
/// rows share few values; V, a value, null for every 17th; S, a string.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (A INTEGER, B INTEGER, V INTEGER, S VARCHAR(12)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (A INTEGER, B INTEGER, V INTEGER, S VARCHAR(12))",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..30_000)
            .map(|k| {
                let v = if k % 17 == 0 { "NULL".to_owned() } else { (k % 101).to_string() };
                format!("({}, {}, {v}, 's{}')", k * 7919 % 25_000, k.div_euclid(750), k * 7919 % 25_000)
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Verifies: [X3.135-1986 §5.22 GR2] (RULES.md D1119) a grouping by keys
/// nearly all apart in each morsel, some repeated across and within
/// morsels, gives in parallel the groups, counts, sums, least and
/// greatest values and the warning of a null eliminated it gives alone,
/// by an integer and by a string, of either store; and so does a
/// grouping by keys in runs, whose morsels are grouped as before.
#[test]
fn keys_apart_in_a_morsel_group_as_they_do_alone() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT A, COUNT(*), SUM(V), MIN(V), MAX(S) FROM {t} GROUP BY A",
            "SELECT S, COUNT(V) FROM {t} GROUP BY S",
            "SELECT B, COUNT(*), SUM(V) FROM {t} GROUP BY B",
            "SELECT N, COUNT(*) FROM (SELECT A, COUNT(*) AS N FROM {t} GROUP BY A) AS X GROUP BY N",
        ] {
            let sql = shape.replace("{t}", table);
            for set in ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"] {
                exec(&a, set).unwrap();
            }
            let mut alone = rows(&a, &sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
            let warned_alone = a.warning();
            for set in [
                "SET EXECUTOR.BATCHES = 'ON'",
                "SET EXECUTOR.DEGREE = 4",
                "SET EXECUTOR.MORSEL_ROWS = 8192",
                "SET EXECUTOR.PARALLEL_MIN_ROWS = 0",
            ] {
                exec(&a, set).unwrap();
            }
            let mut together = rows(&a, &sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
            assert_eq!(a.warning(), warned_alone, "{sql}");
            alone.sort();
            together.sort();
            assert!(!alone.is_empty(), "{sql}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}
