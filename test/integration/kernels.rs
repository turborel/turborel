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
