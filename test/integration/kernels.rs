//! \[DESIGN\] The kernels (RULES.md D354): the portable ones keep the
//! contract every implementation keeps (`support/src/kernels.rs`), and a
//! database runs its batches on the kernels it is given.

use std::sync::Arc;

use turborel::db::Database;
use turborel::kernels::{Kernels, PORTABLE, Portable};

use crate::common::{both_ways, exec};
use crate::kernel_contract::contract;

/// Verifies: [DESIGN] RULES.md D354 each portable kernel gives what a plain
/// loop gives, at every length to past two mask words, of every operator,
/// nulls and selections, and the extremes of each type.
#[test]
fn portable_kernels_keep_the_contract() {
    assert_eq!(Portable.name(), PORTABLE);
    contract(&Portable);
}

/// Kernels that count their selections, each the portable one's, and
/// their decimals decoded and sort keys made.
struct Counting(Arc<std::sync::atomic::AtomicUsize>);

impl Kernels for Counting {
    fn name(&self) -> &'static str {
        "COUNTING"
    }

    fn select_i64(&self, values: &[i64], pred: (turborel::value::CompOp, i64), given: turborel::kernels::Given<'_>, out: &mut Vec<u32>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.select_i64(values, pred, given, out);
    }

    fn alp_decode(&self, words: &mut [i64], scale: turborel::kernels::AlpScale) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.alp_decode(words, scale);
    }

    fn normalize_i64(&self, given: (&[i64], &[u32]), shape: (i64, usize, bool), place: (usize, usize), keys: &mut [u8]) -> Option<()> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.normalize_i64(given, shape, place, keys)
    }
}

/// Verifies: [DESIGN] RULES.md D354, D460 a database whose kernels are
/// registered (`Database::register_kernels`) judges its batches' filters by
/// them, with the rows the portable ones keep; a second set is refused
/// (SQLCODE -910); EXECUTOR.KERNELS = 'PORTABLE' runs the portable ones.
#[test]
fn a_database_runs_the_kernels_registered() {
    let db = Database::new();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    db.register_kernels(Box::new(Counting(Arc::clone(&count)))).unwrap();
    let again = db.register_kernels(Box::new(Portable)).unwrap_err();
    assert_eq!(again.sqlcode(), -910, "{again}");
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..3000).map(|k| format!("({k}, {})", k % 17)).collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    let (batched, by_rows) = both_ways(&a, "SELECT COUNT(*), SUM(K) FROM T WHERE N < 5");
    assert_eq!(batched, by_rows);
    let counted = count.load(std::sync::atomic::Ordering::Relaxed);
    assert!(counted > 0, "no selection by the kernels registered");
    exec(&a, "SET EXECUTOR.KERNELS = 'PORTABLE'").unwrap();
    let (forced, _) = both_ways(&a, "SELECT COUNT(*), SUM(K) FROM T WHERE N < 5");
    assert_eq!(forced, batched);
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), counted, "PORTABLE ran the registered kernels");
}

/// Verifies: [DESIGN] RULES.md D570 the kernels registered reach the
/// column store, its scans' filters judged and its decimals decoded by
/// them, a table opened after they are registered; and a sort's keys are
/// made by them unless EXECUTOR.KERNELS = 'PORTABLE', which no sort then
/// passes by.
#[test]
fn the_kernels_registered_reach_storage_and_sorts() {
    use std::sync::atomic::Ordering::Relaxed;
    let db = Database::new();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    db.register_kernels(Box::new(Counting(Arc::clone(&count)))).unwrap();
    db.set_job_policy(turborel::threads::JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE C (K INTEGER, F DOUBLE PRECISION) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..5000).map(|k| format!("({k}, {}.25)", k % 300)).collect();
    a.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    let (filtered, by_rows) = both_ways(&a, "SELECT COUNT(*), SUM(F) FROM C WHERE K < 1000");
    assert_eq!(filtered, by_rows);
    assert!(count.load(Relaxed) > 0, "no kernel registered ran in the column store");
    let sorted = |a: &turborel::db::Session| {
        let before = count.load(Relaxed);
        let rows = crate::common::sorted(a, "SELECT K FROM C ORDER BY K DESC");
        (rows, count.load(Relaxed) - before)
    };
    let (by_registered, made) = sorted(&a);
    exec(&a, "SET EXECUTOR.KERNELS = 'PORTABLE'").unwrap();
    let (by_portable, made_portable) = sorted(&a);
    assert_eq!(by_registered, by_portable);
    assert!(made > 0, "no sort key made by the kernels registered");
    assert_eq!(made_portable, 0, "PORTABLE ran the registered kernels in a sort");
}

/// Kernels that tally each sum, extreme and gather they make, by name,
/// each the portable one's.
struct Tally(Arc<std::sync::Mutex<std::collections::BTreeMap<&'static str, usize>>>);

impl Tally {
    fn count(&self, name: &'static str) {
        if let Ok(mut tally) = self.0.lock() {
            *tally.entry(name).or_default() += 1;
        }
    }
}

impl Kernels for Tally {
    fn name(&self) -> &'static str {
        "TALLY"
    }

    fn sum_i64(&self, values: &[i64], sel: Option<&[u32]>) -> i128 {
        self.count("sum_i64");
        Portable.sum_i64(values, sel)
    }

    fn min_max_i64(&self, values: &[i64], given: turborel::kernels::Given<'_>) -> Option<(i64, i64)> {
        self.count("min_max_i64");
        Portable.min_max_i64(values, given)
    }

    fn gather_i64(&self, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
        self.count("gather_i64");
        Portable.gather_i64(values, sel, out);
    }
}

/// Verifies: [DESIGN] RULES.md D570 [X3.135-1986 §5.8 GR1]: set functions
/// over a batch's integers sum and compare them on the kernels
/// registered, and a join's, a projection's and a top-N's rows gather
/// them so, of the row store and the column store alike; each query
/// gives what it gives on the portable kernels (EXECUTOR.KERNELS =
/// 'PORTABLE') and read row by row, nulls left out alike.
#[test]
fn set_functions_and_gathers_run_on_the_kernels_registered() {
    let db = Database::new();
    let tally = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
    db.register_kernels(Box::new(Tally(Arc::clone(&tally)))).unwrap();
    db.set_job_policy(turborel::threads::JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER, G INTEGER, V BIGINT, N INTEGER)
           CREATE TABLE C (K INTEGER, G INTEGER, V BIGINT, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE H (K INTEGER, W INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let rows: Vec<String> = (0..6000_i64)
        .map(|k| {
            let n = if k % 7 == 0 { "NULL".to_owned() } else { (k * 31 % 1000 - 500).to_string() };
            format!("({k}, {}, {}, {n})", k % 13, (k * 7_919) % 100_003 - 50_000)
        })
        .collect();
    for table in ["R", "C"] {
        a.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
    }
    let held: Vec<String> = (0..500_i64).map(|k| format!("({}, {k})", k * 11)).collect();
    a.run(&format!("INSERT INTO H VALUES {}", held.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    for table in ["R", "C"] {
        for sql in [
            format!("SELECT SUM(V), MIN(V), MAX(V), SUM(N), MIN(N), MAX(N), AVG(N), COUNT(N) FROM {table}"),
            format!("SELECT G, SUM(V), MIN(N), MAX(N) FROM {table} GROUP BY G"),
            format!("SELECT SUM(V), MAX(N) FROM {table} WHERE K < 3000"),
            format!("SELECT {table}.K, {table}.V, H.W FROM {table} JOIN H ON {table}.K = H.K"),
            format!("SELECT K, N FROM {table} WHERE G = 3"),
            format!("SELECT K, V FROM {table} ORDER BY V DESC FETCH FIRST 20 ROWS ONLY"),
        ] {
            exec(&a, "SET EXECUTOR.KERNELS = ''").unwrap();
            let (registered, by_rows) = both_ways(&a, &sql);
            exec(&a, "SET EXECUTOR.KERNELS = 'PORTABLE'").unwrap();
            let (portable, _) = both_ways(&a, &sql);
            assert_eq!(registered, by_rows, "{sql}");
            assert_eq!(registered, portable, "{sql}");
        }
    }
    let tally = tally.lock().unwrap().clone();
    for name in ["sum_i64", "min_max_i64", "gather_i64"] {
        assert!(tally.get(name).is_some_and(|&n| n > 0), "{name} never ran: {tally:?}");
    }
}
