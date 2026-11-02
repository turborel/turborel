//! VR in the deterministic simulator of `turborel-test-support`: many
//! seeds, each a run of faults at random, checked throughout and then for
//! convergence. A failure names its seed, which makes the run again.

use std::ops::Range;
use std::sync::Arc;

use turborel_test_support::replication::Make;
use turborel_test_support::replication::sim::plans;
use turborel_test_support::replication::sim::{Plan, Report, simulate};

use crate::common::make_vr;

/// Seeds run of each plan: bounded, so `verify` stays quick; longer sweeps
/// take more.
const SEEDS_OF_THREE: Range<u64> = 0..300;
const SEEDS_OF_FIVE: Range<u64> = 0..100;
const SEEDS_OF_STORMS: Range<u64> = 1000..1100;

/// Runs `seeds` of `plan`, and fails naming every seed that failed.
fn sweep(plan: &Plan, seeds: Range<u64>) -> Vec<Report> {
    let make: Make = Arc::new(make_vr);
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    for seed in seeds {
        match simulate(seed, plan, &make) {
            Ok(report) => reports.push(report),
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    reports
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §8.1] in groups of three, through
/// frames delayed, lost, duplicated and reordered, partitions, crashes and
/// restarts, slow disks losing writes not yet durable and failing:
/// committed records are never lost or reordered across view changes; at
/// most one primary serves each view, the one it names; a record in doubt
/// is resolved as the log decided; and once the faults stop, every
/// replica's database converges on the committed log.
#[test]
fn groups_of_three_stay_safe_and_converge() {
    let reports = sweep(&plans::three(), SEEDS_OF_THREE);
    let changed = reports.iter().filter(|r| r.view > 0).count();
    let crashed = reports.iter().filter(|r| r.crashes > 0).count();
    let doubted = reports.iter().filter(|r| r.outcomes > 0).count();
    assert!(changed > 0 && crashed > 0 && doubted > 0, "faults hardly met: {changed} {crashed} {doubted}");
    assert!(reports.iter().all(|r| r.committed > 0));
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §8.1] the same of groups of five,
/// whose quorums are three.
#[test]
fn groups_of_five_stay_safe_and_converge() {
    sweep(&plans::five(), SEEDS_OF_FIVE);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §8.1] the same through storms: a
/// quarter of frames lost, a tenth twice, partitions and crashes several
/// times as often, a disk failing on one write in a hundred.
#[test]
fn groups_stay_safe_through_storms() {
    sweep(&plans::storm(), SEEDS_OF_STORMS);
}

/// Verifies: [DESIGN] RULES.md D57, R-TEST-5 a run is made again exactly
/// from its seed.
#[test]
fn a_seed_makes_its_run_again() {
    let make: Make = Arc::new(make_vr);
    let first = simulate(7, &plans::three(), &make).unwrap();
    assert_eq!(simulate(7, &plans::three(), &make).unwrap(), first);
    assert_ne!(simulate(8, &plans::three(), &make).unwrap(), first);
}
