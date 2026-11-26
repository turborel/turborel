//! \[DESIGN\] FETCH FIRST with no ORDER BY over a grouping (RULES.md
//! D1215): the rows given are groups of the grouping, each whole, as many
//! as are asked for, whether the grouping made only those or every group;
//! and a grouping whose dropped rows could have raised an exception or a
//! warning makes every group still.

use std::collections::HashSet;

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike, of 7000 rows: K, a key; G, one of 500, null for every
/// 41st row, the first among them; M, a number never null; N, a number
/// null for every 13th row; S, one of 3 strings.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "(K INTEGER PRIMARY KEY, G INTEGER, M INTEGER NOT NULL, N INTEGER, S VARCHAR(8) NOT NULL)";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R {columns}"
    ))
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..7000)
            .map(|k| format!("({k}, {}, {}, {}, 's{}')", null_or(k % 41 == 0, k * 7 % 500), k % 97, null_or(k % 13 == 0, k % 100), k % 3))
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Sets `s` to run a plan alone, by batch.
fn alone(s: &Session) {
    for set in ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'ON'"] {
        exec(s, set).unwrap();
    }
}

/// Sets `s` to run a plan on 4 threads in morsels of 256 rows.
fn together(s: &Session) {
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
}

/// Asserts the rows of `grouped` with `fetch` after it, which skips
/// `skipped` rows and asks for `asked`, are that many, or as many as
/// `grouped` has left, no two alike, each a row of `grouped` run whole,
/// of which it has `least` at least.
fn whole_groups(s: &Session, grouped: &str, (fetch, skipped, asked): (&str, usize, usize), least: usize) {
    let every: HashSet<String> = rows(s, grouped).unwrap().into_iter().collect();
    assert!(every.len() >= least, "{grouped}: {} groups", every.len());
    let count = asked.min(every.len().saturating_sub(skipped));
    let sql = format!("{grouped} {fetch}");
    let given = rows(s, &sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    assert_eq!(given.len(), count, "{sql}: {given:?}");
    assert_eq!(given.iter().collect::<HashSet<_>>().len(), count, "{sql}: a group given twice");
    for row in &given {
        assert!(every.contains(row), "{sql}: {row} is no whole group");
    }
}

/// The groupings whose first groups alone may be made, with the least
/// groups each has.
const SHAPES: [(&str, usize); 6] = [
    ("SELECT G, COUNT(*) FROM {t} GROUP BY G", 501),
    ("SELECT G, S, COUNT(*), MIN(M), MAX(M), SUM(M), COUNT(M) FROM {t} GROUP BY G, S", 1500),
    ("SELECT COUNT(*), S, G FROM {t} GROUP BY S, G", 1500),
    ("SELECT G, COUNT(*) FROM {t} WHERE K >= 300 AND M <> 5 GROUP BY G", 500),
    ("SELECT S, MAX(K) FROM {t} GROUP BY S", 3),
    ("SELECT M, G, MIN(S) FROM {t} WHERE G IS NULL OR G < 40 GROUP BY M, G", 100),
];

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR5a] [§7.17 GR6] [§7.17 GR7d ii] [§7.13 GR2b]
/// (RULES.md D1215) FETCH FIRST with no ORDER BY over a grouping gives
/// that many of its groups, each as the grouping run whole gives it, of
/// the column store and the row store, run alone and in parallel: of one
/// group, of some, the null group among the first, of an OFFSET added to
/// the count, of a count past the groups there are, and under a WHERE.
#[test]
fn the_first_groups_given_are_whole() {
    let (_db, a) = tables();
    for (table, parallel) in [("C", false), ("C", true), ("R", false), ("R", true)] {
        if parallel {
            together(&a)
        } else {
            alone(&a)
        }
        for (shape, least) in SHAPES {
            let sql = shape.replace("{t}", table);
            for fetch in [
                ("FETCH FIRST 1 ROW ONLY", 0, 1),
                ("FETCH FIRST 5 ROWS ONLY", 0, 5),
                ("OFFSET 2 ROWS FETCH FIRST 7 ROWS ONLY", 2, 7),
                ("FETCH FIRST 2000 ROWS ONLY", 0, 2000),
            ] {
                whole_groups(&a, &sql, fetch, least);
            }
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR5a] (RULES.md D1215) a
/// grouping run in parallel under FETCH FIRST is a part still, as EXPLAIN
/// shows, and gives the groups it gives run alone, those of the first
/// rows read, in their order: the null group, of the first row, with
/// every row of it, though its rows are in every morsel.
#[test]
fn a_part_gives_the_groups_of_its_first_rows() {
    let (_db, a) = tables();
    for sql in [
        "SELECT G, COUNT(*), MAX(K) FROM C GROUP BY G FETCH FIRST 6 ROWS ONLY",
        "SELECT G, S, COUNT(*) FROM C GROUP BY G, S OFFSET 1 ROW FETCH FIRST 4 ROWS ONLY",
        "SELECT S, G, MIN(M) FROM C WHERE K > 10 GROUP BY S, G FETCH FIRST 300 ROWS ONLY",
    ] {
        alone(&a);
        let serial = rows(&a, sql).unwrap();
        together(&a);
        let shown = a.physical_plan(sql, &Params::new()).unwrap();
        assert!(shown.contains("Parallel"), "{sql}\n{shown}");
        assert_eq!(rows(&a, sql).unwrap(), serial, "{sql}");
    }
    let null_group = rows(&a, "SELECT G, COUNT(*) FROM C GROUP BY G FETCH FIRST 1 ROW ONLY").unwrap();
    assert_eq!(null_group, rows(&a, "SELECT G, COUNT(*) FROM C WHERE G IS NULL GROUP BY G").unwrap());
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR5a] [§7.13 SR] [§10.9 GR7b i] (RULES.md D1215)
/// a grouping whose first groups alone are not made gives whole groups
/// too: under a HAVING condition, which is judged of whole groups, each
/// row given one it keeps; of grouping sets, whose groups are made
/// before their rows; of a DISTINCT set function; and of the groups'
/// order asked, a sort between.
#[test]
fn a_grouping_not_asked_for_fewer_gives_whole_groups() {
    let (_db, a) = tables();
    for parallel in [false, true] {
        if parallel {
            together(&a)
        } else {
            alone(&a)
        }
        for sql in [
            "SELECT G, COUNT(*) FROM C GROUP BY G HAVING COUNT(*) > 13",
            "SELECT G, S, COUNT(*) FROM C GROUP BY ROLLUP (G, S)",
            "SELECT G, COUNT(DISTINCT M) FROM C GROUP BY G",
            "SELECT G, COUNT(*) FROM R GROUP BY G HAVING MAX(K) > 6990",
        ] {
            whole_groups(&a, sql, ("FETCH FIRST 5 ROWS ONLY", 0, 5), 5);
        }
        let sorted = "SELECT G, COUNT(*) FROM C GROUP BY G ORDER BY 2 DESC, 1";
        let every = rows(&a, sorted).unwrap();
        assert_eq!(rows(&a, &format!("{sorted} FETCH FIRST 5 ROWS ONLY")).unwrap(), every[..5], "the first by the order");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7a] [§10.9 GR7d vi] [§6.30 GR5] (RULES.md
/// D1215, D15) a grouping whose rows past its first groups could raise
/// is read whole under FETCH FIRST, and raises what it raises without
/// it: a null eliminated in a later group is the warning 01003; a sum
/// past the range of its type in a later group is 22003; a division by
/// zero in a later row's argument is 22012.
#[test]
fn a_grouping_that_could_raise_reads_every_row() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE W (K INTEGER PRIMARY KEY, G INTEGER NOT NULL, N INTEGER, D INTEGER NOT NULL, B DECIMAL(38, 0) NOT NULL)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let big = format!("6{}", "0".repeat(37));
    let values: Vec<String> = (0_i64..3000)
        .map(|k| {
            let last = k >= 2998;
            format!(
                "({k}, {}, {}, {}, {})",
                k / 2,
                if last { "NULL".into() } else { k.to_string() },
                i64::from(!last),
                if last { &big } else { "1" }
            )
        })
        .collect();
    a.run(&format!("INSERT INTO W VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    for parallel in [false, true] {
        if parallel {
            together(&a)
        } else {
            alone(&a)
        }
        let nulls = "SELECT G, SUM(N) FROM W GROUP BY G FETCH FIRST 3 ROWS ONLY";
        assert_eq!(rows(&a, nulls).map(|r| r.len()), Ok(3), "{nulls}");
        assert_eq!(a.warning().as_ref().map(SqlState::as_str), Some("01003"), "{nulls}: a null eliminated in the last group");
        let quiet = "SELECT G, SUM(D), COUNT(*) FROM W GROUP BY G FETCH FIRST 3 ROWS ONLY";
        assert_eq!(rows(&a, quiet).map(|r| r.len()), Ok(3), "{quiet}");
        assert_eq!(a.warning(), None, "{quiet}: no null among its values");
        for (sql, state) in [
            ("SELECT G, SUM(B) FROM W GROUP BY G FETCH FIRST 3 ROWS ONLY", "22003"),
            ("SELECT G, SUM(K / D) FROM W GROUP BY G FETCH FIRST 3 ROWS ONLY", "22012"),
        ] {
            let failed = crate::common::failed(&a, sql);
            assert_eq!(failed.sqlstate().as_str(), state, "{sql}: {failed}");
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.13 GR2b] (RULES.md D1215, D38) a
/// grouping asked for more groups than its work area holds makes the
/// groups past it of every row of theirs, none dropped once a group has
/// been written past the area: each row given is a whole group.
#[test]
fn groups_past_the_work_area_are_whole() {
    let (db, a) = tables();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
    alone(&a);
    let wide = "SELECT K, G, S, COUNT(*), MAX(M) FROM C GROUP BY K, G, S";
    whole_groups(&a, wide, ("FETCH FIRST 6900 ROWS ONLY", 0, 6900), 7000);
    let few = "SELECT G, S, COUNT(*), MAX(M) FROM C GROUP BY G, S";
    whole_groups(&a, few, ("FETCH FIRST 1400 ROWS ONLY", 0, 1400), 1500);
}
