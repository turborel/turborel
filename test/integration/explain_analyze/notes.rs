//! \[EXTENSION\] What an operator EXPLAIN ANALYZE counts shows beside its
//! rows (RULES.md D620, D621): its batches and their mean rows, what it
//! spilled past its work area, and what it read of an index.

use super::{analyzed, database, op};
use crate::common::exec;

/// Verifies: [EXTENSION] RULES.md D621: a scan run by batches shows its
/// batches and their mean rows; run a row at a time, none.
#[test]
fn a_scan_by_batches_shows_its_batches() {
    let db = database();
    let s = db.session("ANN");
    exec(&s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    let a = analyzed(&s, "SELECT K FROM T");
    let scan = op(&a, "SeqScan ANN.T");
    assert!(scan.notes.batches >= 1, "{}", a.text);
    let mean = scan.rows.div_ceil(scan.notes.batches);
    assert!(a.text.contains(&format!("batches={}, mean rows=", scan.notes.batches)), "{}", a.text);
    assert!(mean >= 1, "{}", a.text);
    exec(&s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rowed = analyzed(&s, "SELECT K FROM T");
    assert_eq!(op(&rowed, "SeqScan ANN.T").notes.batches, 0, "{}", rowed.text);
    assert!(!rowed.text.contains("batches="), "{}", rowed.text);
}

/// Verifies: [EXTENSION] RULES.md D621: an index scan shows the entries
/// it took, the leaf pages of its tree it read, and the rows it fetched;
/// a sequential scan no index reads.
#[test]
fn an_index_scan_shows_its_reads() {
    let db = database();
    let s = db.session("ANN");
    let a = analyzed(&s, "SELECT V FROM T WHERE K BETWEEN 10 AND 19");
    let scan = op(&a, "IndexScan ANN.T");
    assert_eq!((scan.rows, scan.notes.entries, scan.notes.fetched, scan.notes.rechecked), (10, 10, 10, 0), "{}", a.text);
    assert!(scan.notes.leaves >= 1, "{}", a.text);
    assert!(a.text.contains("entries=10, leaf pages=") && a.text.contains("fetched=10"), "{}", a.text);
    let seq = analyzed(&s, "SELECT V FROM T WHERE V > 990");
    assert_eq!(op(&seq, "SeqScan ANN.T").notes.entries, 0, "{}", seq.text);
}

/// Verifies: [EXTENSION] RULES.md D620: past a small work area, a sort and
/// a grouping each show what they spilled, the runs and partitions they
/// wrote, and the passes they read back; in a large one, nothing spilled.
#[test]
fn operators_past_their_work_area_show_what_they_spilled() {
    let db = database();
    let s = db.session("ANN");
    let sorted = "SELECT T.V, S.X FROM T, S ORDER BY T.V + S.K * 1000";
    let grouped = "SELECT G, COUNT(*) FROM (SELECT T.V * 1000 + S.K AS G FROM T, S) AS D GROUP BY G";
    let in_memory = analyzed(&s, sorted);
    assert_eq!(op(&in_memory, "Sort").notes.spilled, 0, "{}", in_memory.text);
    assert!(!in_memory.text.contains("spilled="), "{}", in_memory.text);
    exec(&s, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for (sql, word) in [(sorted, "Sort"), (grouped, "Aggregate")] {
        let a = analyzed(&s, sql);
        assert_eq!(a.rows, 100_000, "{}", a.text);
        let spilled = op(&a, word).notes;
        assert!(spilled.spilled > 0 && spilled.partitions > 0 && spilled.passes >= 1, "{word}: {}", a.text);
        assert!(a.text.contains("spilled=") && a.text.contains(" bytes, partitions="), "{}", a.text);
    }
}
