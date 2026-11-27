//! \[DESIGN\] Hash joins run in parallel whose built operand outgrows the
//! grant (RULES.md D1240): built in parts and probed in passes, each
//! probing row in one pass, so an inner join, a LEFT join, a semi join
//! and an antijoin give the rows the plan run alone gives; and of a grant
//! so small the passes would be too many, given way to the plan run
//! alone, nothing of its parts left held.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, sorted, spill_bytes};

/// A grant the built operand outgrows a few times, 1 MiB.
const PASSED: i128 = 1 << 20;

/// The smallest MEMORY.WORK_BYTES, 64 KiB, which it outgrows many times.
const SMALL: i128 = 64 << 10;

/// Tables D, 8000 rows of K, G one of 2000, null for every 97th, and a
/// text of 120 characters; E, 8000 rows of K, G one of 8, and the text,
/// so a pass holds a key or two and its keys' range is not the whole
/// operand's; and F, 6000 rows of K and G one of 2500, null for every
/// 89th.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE D (K INTEGER, G INTEGER, T VARCHAR(200))
           CREATE TABLE F (K INTEGER, G INTEGER)
           CREATE TABLE E (K INTEGER, G INTEGER, T VARCHAR(200))",
    )
    .unwrap();
    let a = db.session("A");
    for from in (0_i64..8_000).step_by(500) {
        let d: Vec<String> = (from..from + 500)
            .map(|k| {
                let g = if k % 97 == 0 { "NULL".to_owned() } else { (k * 7 % 2_000).to_string() };
                format!("({k}, {g}, '{}')", format!("{k:08}").repeat(15))
            })
            .collect();
        a.run(&format!("INSERT INTO D VALUES {}", d.join(", "))).unwrap();
        let e: Vec<String> = (from..from + 500).map(|k| format!("({k}, {}, '{}')", k % 8 + 1, format!("{k:08}").repeat(15))).collect();
        a.run(&format!("INSERT INTO E VALUES {}", e.join(", "))).unwrap();
    }
    let f: Vec<String> =
        (0_i64..6_000).map(|k| if k % 89 == 0 { format!("({k}, NULL)") } else { format!("({k}, {})", k * 3 % 2_500) }).collect();
    a.run(&format!("INSERT INTO F VALUES {}", f.join(", "))).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, sorted, run alone on one thread.
fn alone(s: &Session, sql: &str) -> Vec<String> {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    sorted(s, sql)
}

/// The rows of `sql`, sorted, run in parallel on 4 threads in morsels of
/// 256 rows, its plan asserted to run a part in parallel.
fn parallel(s: &Session, sql: &str) -> Vec<String> {
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap();
    assert!(shown.contains("Parallel"), "{sql}: run in parallel\n{shown}");
    sorted(s, sql)
}

/// An inner join, a LEFT join, a semi join and an antijoin of F with D on
/// their G, D built; and an inner join with E, whose passes' keys each
/// lie in a range of their own, so its scan of F is told the range of the
/// whole operand's keys, not the first pass's.
const QUERIES: [&str; 5] = [
    "SELECT F.K, E.K FROM F JOIN E ON F.G = E.G",
    "SELECT F.K, D.K, D.T FROM F JOIN D ON F.G = D.G",
    "SELECT F.K, D.K FROM F LEFT JOIN D ON F.G = D.G",
    "SELECT F.K FROM F WHERE F.G IN (SELECT D.G FROM D)",
    "SELECT F.K FROM F WHERE NOT EXISTS (SELECT * FROM D WHERE D.G = F.G)",
];

/// Verifies: [DESIGN] (RULES.md D1240) hash joins run in parallel whose
/// built operand outgrows the grant, of each kind, give the rows they give
/// run alone, by batch and row by row: built in passes at a grant a few
/// times smaller than the operand, given way at the smallest, and of
/// either nothing left held of the budget's spill areas once each ends.
#[test]
fn joins_built_past_the_grant_give_their_rows() {
    let (db, a) = tables();
    let expected: Vec<Vec<String>> = QUERIES.iter().map(|q| alone(&a, q)).collect();
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        for grant in [PASSED, SMALL] {
            db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(grant, 0))).unwrap();
            for (q, want) in QUERIES.iter().zip(&expected) {
                assert!(!want.is_empty(), "{q}");
                assert_eq!(&parallel(&a, q), want, "{q}, grant {grant}, batches {batches}");
                assert_eq!(spill_bytes(&db), 0, "{q}: nothing left held");
            }
        }
    }
}
