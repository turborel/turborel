//! Unit tests of a run of morsels dropped by its reader (RULES.md D290,
//! D1245).

use std::sync::Arc;
use std::time::Duration;

use super::{Morsels, Work};
use crate::db::Database;
use crate::value::{Decimal, Value};

/// Verifies: [DESIGN] (RULES.md D290, D1245) Regression test (R-TEST-7):
/// a run dropped by its reader after its first result left its work, and
/// what the work held, to the pool's task until a worker next woke, so a
/// statement returned with its workers still holding its lists. Dropped
/// now, it waits for the morsels running and lets go of its work and of
/// the results not read before the drop returns: each morsel and each
/// result holds the token, and only the test's own is left.
#[test]
fn a_run_dropped_lets_go_of_its_work_and_its_results_at_once() {
    let db = Database::new();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let (st, _) = db.session("A").optimized("SELECT K FROM T").unwrap();
    let token = Arc::new(());
    for _ in 0_usize..20 {
        let held = Arc::clone(&token);
        let work: Work<Arc<()>> = Box::new(move |_m: usize| {
            let running = Arc::clone(&held);
            std::thread::sleep(Duration::from_millis(1));
            Ok(running)
        });
        let mut run = Morsels::start(&st, crate::txn::NO_TRANSACTION, (1_000, 4), work);
        assert!(run.next().is_some_and(|first| first.is_ok()));
        drop(run);
        assert_eq!(Arc::strong_count(&token), 1, "nothing of the run left held");
    }
}
