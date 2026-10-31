//! Real databases in the deterministic simulator (RULES.md D57): each VR
//! replica keeps a turborel database, which follows its replica's log
//! through the core's `Standby` seam and serves SQL while its replica is
//! primary. After runs of faults at random, every backup must hold what
//! the primary holds; and a backup promoted by a view change serves.

use std::ops::Range;
use std::sync::Arc;

use turborel_test_support::database::rows;
use turborel_test_support::replication::Make;
use turborel_test_support::replication::sim::{DatabaseReport, Faults, Plan, Replicas, Weather, simulate_databases};

use crate::common::make_vr;

/// Seeds run: bounded, as each replica's database is a real one.
const SEEDS: Range<u64> = 0..40;

/// A run of three replicas: 2000 moments of faults, 2000 calm; the
/// primary's database acting at one moment in five; a replica ticking
/// every 5. Of the frames, 5 % are lost, 2 % delivered twice, 1 % held up
/// to 300 moments, the rest taking up to 10; a partition begins at 3
/// moments in a thousand, a replica crashes at 4 in a thousand and stays
/// down up to 60, a disk fails on 2 writes in a thousand.
fn plan() -> Plan {
    let weather = Weather { drop: 50, duplicate: 20, delay: 10, late: 10, late_delay: 300 };
    let faults = Faults { weather, partition: 3, partition_moments: 400, crash: 4, down_moments: 60, disk_error: 2, write_moments: 6 };
    Plan { replicas: 3, faulty: 2000, calm: 2000, append: 200, tick_moments: 5, faults }
}

/// A run with no fault at all, long enough never to end in a test.
fn calm_plan() -> Plan {
    let weather = Weather { drop: 0, duplicate: 0, delay: 3, late: 0, late_delay: 1 };
    let faults = Faults { weather, partition: 0, partition_moments: 1, crash: 0, down_moments: 20, disk_error: 0, write_moments: 2 };
    Plan { replicas: 3, faulty: 0, calm: 1_u64 << 40, append: 0, tick_moments: 5, faults }
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1] [VRR §4.2] in groups of three
/// replicas each keeping a real database, through frames delayed, lost,
/// duplicated and reordered, partitions, crashes and failing disks, while
/// the primary's database runs transactions spanning moments: after the
/// calm, every backup has replayed the committed log and holds what the
/// primary holds, the same rows, index entries and Information Schema;
/// and the faults were met: commits in doubt, transactions left open by a
/// primary stopped, and backups promoted.
#[test]
fn backups_hold_what_the_primary_holds() {
    let make: Make = Arc::new(make_vr);
    let mut reports: Vec<DatabaseReport> = Vec::new();
    let mut failures = Vec::new();
    for seed in SEEDS {
        match simulate_databases(seed, &plan(), &make) {
            Ok(report) => reports.push(report),
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    assert!(reports.iter().all(|r| r.compared == 2 && r.committed > 0), "{reports:?}");
    let met = |f: fn(&DatabaseReport) -> bool| reports.iter().filter(|&r| f(r)).count();
    let (doubted, interrupted, promoted) = (met(|r| r.in_doubt > 0), met(|r| r.interrupted > 0), met(|r| r.promoted > 0));
    assert!(doubted > 0 && interrupted > 0 && promoted > 0, "faults hardly met: {doubted} {interrupted} {promoted}");
}

/// Verifies: [DESIGN] RULES.md D57, R-TEST-5 a run of real databases is
/// made again exactly from its seed.
#[test]
fn a_seed_makes_its_run_of_databases_again() {
    let make: Make = Arc::new(make_vr);
    let first = simulate_databases(3, &plan(), &make).unwrap();
    assert_eq!(simulate_databases(3, &plan(), &make).unwrap(), first);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.2] a view change promoting a
/// backup: the primary's database commits rows and leaves a transaction
/// open, whose records its backups are given; the primary crashes; a
/// backup's replica is the next view's primary, and its database, promoted,
/// has ended the transaction left open and serves: it commits a row, which
/// the other replicas' databases, the crashed one made again among them,
/// then hold, as they hold all it holds.
#[test]
fn a_promoted_backup_serves() {
    let make: Make = Arc::new(make_vr);
    let mut group = Replicas::new(1, &calm_plan(), &make);
    group.moments(50).unwrap();
    assert_eq!(group.primary(), Some(0));
    let first = group.database(0).clone();
    first.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)").unwrap();
    let s = first.session("APP");
    s.run("INSERT INTO T VALUES (1, 10)").unwrap();
    s.run("INSERT INTO T VALUES (2, 20)").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (3, 30)").unwrap();
    group.moments(50).unwrap();
    assert_eq!(rows(&group.database(1).session("APP"), "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2"]);

    group.crash(0).unwrap();
    drop((s, first));
    group.moments(300).unwrap();
    let primary = group.primary().unwrap();
    assert_ne!(primary, 0, "a backup serves the next view");
    let serving = group.database(primary).clone();
    let writer = serving.session("APP");
    assert_eq!(rows(&writer, "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2"], "the transaction left open ended");
    writer.run("INSERT INTO T VALUES (4, 40)").unwrap();
    writer.commit().unwrap();
    group.moments(300).unwrap();
    let report = group.compare().unwrap();
    assert_eq!((report.compared, report.promoted), (2, 1));
    assert_eq!(rows(&group.database(0).session("APP"), "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2", "4"]);
}
