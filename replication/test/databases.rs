//! Real databases in the deterministic simulator (RULES.md D57): each VR
//! replica has the crate's driver over a turborel database kept in files,
//! which follows its replica's log through the core's `Standby` seam,
//! serves SQL while its replica is primary, is opened again in place once
//! deposed, and from its last checkpoint once crashed. After runs of
//! faults at random, every backup must hold what the primary holds; and a
//! backup promoted by a view change serves.

use std::ops::Range;
use std::sync::Arc;

use turborel_test_support::database::rows;
use turborel_test_support::replication::Make;
use turborel_test_support::replication::sim::plans;
use turborel_test_support::replication::sim::{DatabaseReport, Faults, Plan, Replicas, Weather, simulate_databases};

use crate::common::make_vr;

/// Seeds run: bounded, as each replica's database is a real one.
const SEEDS: Range<u64> = 0..40;

/// A run with no fault at all, long enough never to end in a test.
fn calm_plan() -> Plan {
    let weather = Weather { drop: 0, duplicate: 0, delay: 3, late: 0, late_delay: 1 };
    let faults = Faults {
        weather,
        partition: 0,
        partition_moments: 1,
        crash: 0,
        down_moments: 20,
        disk_error: 0,
        lose_journal: 0,
        write_moments: 2,
    };
    Plan { replicas: 3, faulty: 0, calm: 1_u64 << 40, append: 0, tick_moments: 5, retain_bytes: u64::MAX, faults }
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1] [VRR §4.2] in groups of three
/// replicas each keeping a real database driven by the crate's driver,
/// through frames delayed, lost, duplicated and reordered, partitions,
/// crashes and failing disks, while the primary's database runs
/// transactions spanning moments and the databases take checkpoints:
/// after the calm, every backup holds the committed log and what the
/// primary holds, the same rows, index entries and Information Schema;
/// and the faults were met: commits in doubt, transactions left open by a
/// primary stopped, backups promoted, checkpoints of following databases,
/// databases opened again from their files after a crash, which lost the
/// writes not synced, journals lost and recovered, state transfers, and
/// reads served by backups, each holding its snapshot a while.
#[test]
fn backups_hold_what_the_primary_holds() {
    let make: Make = Arc::new(make_vr);
    let mut reports: Vec<DatabaseReport> = Vec::new();
    let mut failures = Vec::new();
    for seed in SEEDS {
        match simulate_databases(seed, &plans::databases(), &make) {
            Ok(report) => reports.push(report),
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    assert!(reports.iter().all(|r| r.compared == 2 && r.committed > 0), "{reports:?}");
    let met = |f: fn(&DatabaseReport) -> bool| reports.iter().filter(|&r| f(r)).count();
    let (doubted, interrupted, promoted) = (met(|r| r.in_doubt > 0), met(|r| r.interrupted > 0), met(|r| r.promoted > 0));
    assert!(doubted > 0 && interrupted > 0 && promoted > 0, "faults hardly met: {doubted} {interrupted} {promoted}");
    let (checkpointed, reopened) = (met(|r| r.following_checkpoints > 0), met(|r| r.reopened > 0));
    assert!(checkpointed > 0 && reopened > 0, "recovery hardly met: {checkpointed} {reopened}");
    let (lost, transferred, read) =
        (met(|r| r.replication.journals_lost > 0), met(|r| r.replication.transfers > 0), met(|r| r.backup_reads > 0));
    assert!(lost > 0 && transferred > 0 && read > 0, "journals lost, transfers and backup reads hardly met: {lost} {transferred} {read}");
}

/// Verifies: [DESIGN] RULES.md D57, R-TEST-5 a run of real databases is
/// made again exactly from its seed.
#[test]
fn a_seed_makes_its_run_of_databases_again() {
    let make: Make = Arc::new(make_vr);
    let first = simulate_databases(3, &plans::databases(), &make).unwrap();
    assert_eq!(simulate_databases(3, &plans::databases(), &make).unwrap(), first);
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
    let mut group = Replicas::new(1, &calm_plan(), &make).unwrap();
    group.moments(50).unwrap();
    assert_eq!(group.primary(), Some(0));
    let first = group.database(0).unwrap();
    first.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)").unwrap();
    let s = first.session("APP");
    s.run("INSERT INTO T VALUES (1, 10)").unwrap();
    s.run("INSERT INTO T VALUES (2, 20)").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (3, 30)").unwrap();
    group.moments(50).unwrap();
    assert_eq!(rows(&group.database(1).unwrap().session("APP"), "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2"]);

    group.crash(0).unwrap();
    drop((s, first));
    group.moments(300).unwrap();
    let primary = group.primary().unwrap();
    assert_ne!(primary, 0, "a backup serves the next view");
    let serving = group.database(primary).unwrap();
    let writer = serving.session("APP");
    assert_eq!(rows(&writer, "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2"], "the transaction left open ended");
    writer.run("INSERT INTO T VALUES (4, 40)").unwrap();
    writer.commit().unwrap();
    group.moments(300).unwrap();
    let report = group.compare().unwrap();
    assert_eq!((report.compared, report.promoted), (2, 1));
    assert_eq!(rows(&group.database(0).unwrap().session("APP"), "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2", "4"]);
}
