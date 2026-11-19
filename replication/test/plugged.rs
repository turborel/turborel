//! Replication served by host plug-ins (RULES.md D270 to D274) in the
//! deterministic simulator: each replica's transport, journal, state
//! transfer and consensus the crate's bridge over a stand-in program,
//! killed at moments each seed chooses. A death fails its replica stop;
//! the group must stay safe and converge as through any crash.

use std::ops::Range;
use std::sync::Arc;

use turborel_test_support::replication::Make;
use turborel_test_support::replication::sim::plans;
use turborel_test_support::replication::sim::{Part, Plan, Plugging, Report, simulate_plugged};

use crate::common::make_vr;

/// Seeds run of each plan, bounded so `verify` stays quick.
const SEEDS: Range<u64> = 0..40;
const SEEDS_OF_TRANSFERS: Range<u64> = 0..20;

/// Runs `seeds` of `plan` plugged as `plugging`, and fails naming every
/// seed that failed.
fn sweep(plan: &Plan, seeds: Range<u64>, plugging: Plugging) -> Vec<Report> {
    let make: Make = Arc::new(make_vr);
    let (mut reports, mut failures) = (Vec::new(), Vec::new());
    for seed in seeds {
        match simulate_plugged(seed, plan, &make, Some(plugging)) {
            Ok(report) => reports.push(report),
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    reports
}

/// Verifies: [EXTENSION] RULES.md D270, D271, D272, D273: a group whose
/// every part a plug-in serves, no program dying, stays safe and
/// converges as the built-in parts do, each call crossing the bridge.
#[test]
fn a_group_served_by_plugins_converges() {
    let reports = sweep(&plans::three(), SEEDS, Plugging::all(0, 0));
    assert!(reports.iter().all(|r| r.committed > 0 && r.plugin_kills == 0));
}

/// Verifies: [EXTENSION] RULES.md D270, D271, D272: programs of every part
/// killed mid-call, before or after acting: each death fails its replica
/// stop, and the group never diverges nor commits what a journal did not
/// make durable, and converges once the deaths stop.
#[test]
fn plugins_killed_mid_call_fail_stop_and_the_group_converges() {
    let reports = sweep(&plans::three(), SEEDS, Plugging::all(1, 0));
    assert!(reports.iter().map(|r| r.plugin_kills).sum::<u64>() > 0, "no stand-in was killed");
}

/// Verifies: [EXTENSION] RULES.md D273: a plug-in consensus killed while
/// it changes views, and made again from its journal: the replicas agree
/// on every committed record and converge.
#[test]
fn a_plugin_consensus_killed_mid_view_change_is_remade_and_agrees() {
    let plugging = Plugging { kill: 0, view_change_kill: 150, parts: &[Part::Consensus] };
    let reports = sweep(&plans::storm(), SEEDS, plugging);
    assert!(reports.iter().map(|r| r.plugin_kills).sum::<u64>() > 0, "no consensus was killed in a view change");
    assert!(reports.iter().any(|r| r.view > 0));
}

/// Verifies: [EXTENSION] RULES.md D272: state transfers through a plug-in
/// killed mid-transfer: the fetch is begun again by the replica made
/// again, and nothing half fetched is installed.
#[test]
fn a_plugin_transfer_killed_mid_fetch_begins_again() {
    let plugging = Plugging { kill: 20, view_change_kill: 0, parts: &[Part::Transfer] };
    let reports = sweep(&plans::retaining(), SEEDS_OF_TRANSFERS, plugging);
    assert!(reports.iter().map(|r| r.transfers).sum::<u64>() > 0, "no state transfer was installed");
    assert!(reports.iter().map(|r| r.plugin_kills).sum::<u64>() > 0, "no transfer was killed");
}
