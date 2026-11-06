//! When a table's statistics are gathered in the background, and what a
//! pass during a transaction leaves to count (RULES.md D31).

use turborel::threads::JobPolicy;

use crate::statistics::{above, added, graded};

/// Verifies: [DESIGN] RULES.md D31, statistics in the background: the
/// database's maintenance thread gathers a table's statistics once it has
/// changed enough, no call asking it to.
#[test]
fn statistics_are_gathered_in_the_background() {
    let db = graded(1000);
    db.set_job_policy(JobPolicy::Automatic).unwrap();
    added(&db, 1001..=1300, |_| 1);
    let until = std::time::Instant::now().checked_add(std::time::Duration::from_secs(30)).unwrap();
    while above(&db, 1000).is_none_or(|e| e < 100.0) {
        assert!(std::time::Instant::now() < until, "gathered within 30 seconds");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Verifies: [DESIGN] RULES.md D31, statistics in the background: a pass
/// made while a transaction is open, which it cannot see the changes of,
/// leaves them to count once it commits, so statistics follow the rows it
/// committed. Without the fix the pass took them as seen, the rest of the
/// transaction's changes fell short of the threshold, and statistics
/// stayed as they were for good, as `statistics_are_gathered_in_the_background`
/// showed on a slow runner.
#[test]
fn a_pass_during_a_transaction_leaves_its_changes_to_count() {
    let db = graded(1000);
    let s = db.session("APP");
    for i in 1001..=1200 {
        s.run(&format!("INSERT INTO T VALUES ({i}, 1, 'N1')")).unwrap();
    }
    db.maintain().unwrap();
    for i in 1201..=1300 {
        s.run(&format!("INSERT INTO T VALUES ({i}, 1, 'N1')")).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    assert!(above(&db, 1000).is_some_and(|e| e > 100.0), "300 rows committed: gathered");
}
