//! The mutations of replication (RULES.md R-TEST-9, D57): Viewstamped
//! Replication's commit, acknowledgement, view change, windows, state
//! transfer and recovery, in the crate `turborel-replication`, whose
//! tests, the simulator's sweeps among them, must catch each. Split from
//! list.rs (RULES.md R-SIZE-2).

use super::{Mutation, Package};

/// The simulator's sweep of three replicas keeping little of the log,
/// which crashes often: what catches a window or a transfer broken.
const TRANSFER_SWEEP: &str = "transfer::replicas_behind_the_log_fetch_state_and_converge";
/// The recovery of a replica whose journal was lost, step by step.
const LOST_JOURNAL: &str = "recovery::a_replica_losing_its_journal_recovers_from_the_primary";
/// A new group bootstrapping by recovery.
const BOOTSTRAP: &str = "recovery::a_new_group_bootstraps_by_recovery";

/// The mutations of replication, in the order they run.
pub const VR: [Mutation; 12] = [
    Mutation {
        name: "vr-ack-before-durable",
        package: Package::Replication,
        breaks: "VR: a backup acknowledges a record before its journal holds it durably",
        file: "replication/src/vr/normal.rs",
        original: "                self.log.push(m.record.clone());\n                self.persist(m.op, m.record, out);",
        replacement: "                self.log.push(m.record.clone());\n                self.persist(m.op, m.record, out);\n                let early = Message::PrepareOk(PrepareOk { view: self.view, op: m.op, replica: self.me });\n                Vr::send(self.primary_of(self.view), &early, out);",
        integration: &["normal::commit_waits_for_a_quorum_holding_the_record_durably"],
        unit: &[],
    },
    Mutation {
        name: "vr-commit-short-of-quorum",
        package: Package::Replication,
        breaks: "VR: the primary commits a record one holder short of a quorum",
        file: "replication/src/vr/normal.rs",
        original: "            if holders < self.quorum() {",
        replacement: "            if holders.saturating_add(1) < self.quorum() {",
        integration: &[
            "normal::one_replica_commits_once_its_record_is_durable",
            "normal::commit_waits_for_a_quorum_holding_the_record_durably",
        ],
        unit: &[],
    },
    Mutation {
        name: "vr-earlier-view-taken",
        package: Package::Replication,
        breaks: "VR: a backup takes a PREPARE or COMMIT of the view before its own",
        file: "replication/src/vr/normal.rs",
        original: "        if view < self.view {\n            return false;\n        }",
        replacement: "        if view.saturating_add(1) < self.view {\n            return false;\n        }",
        integration: &["view_change::messages_of_earlier_views_are_dropped"],
        unit: &[],
    },
    Mutation {
        name: "vr-view-change-oldest-log",
        package: Package::Replication,
        breaks: "VR: a new primary takes the oldest log offered, not the latest",
        file: "replication/src/vr/view_change.rs",
        original: ".max_by_key(|c| (c.normal_view, c.op))",
        replacement: ".min_by_key(|c| (c.normal_view, c.op))",
        integration: &["view_change::a_new_primary_takes_the_latest_longest_log", "simulation::groups_of_three_stay_safe_and_converge"],
        unit: &[],
    },
    Mutation {
        name: "vr-window-past-commit-installed",
        package: Package::Replication,
        breaks: "VR: a window beginning past the replica's commit-number is installed, its records between unverified",
        file: "replication/src/vr/view_change.rs",
        original: "        if self.op() < after || self.commit < after {",
        replacement: "        if self.op() < after {",
        integration: &[TRANSFER_SWEEP],
        unit: &[],
    },
    Mutation {
        name: "vr-window-not-skipped",
        package: Package::Replication,
        breaks: "VR: a window installed from before this one's first position is not skipped through it",
        file: "replication/src/vr/view_change.rs",
        original: "        if after >= first {",
        replacement: "        if after >= first || after < first {",
        integration: &[TRANSFER_SWEEP],
        unit: &[],
    },
    Mutation {
        name: "vr-need-state-unasked",
        package: Package::Replication,
        breaks: "VR: a NEEDSTATE answering an older GETSTATE moves the replica",
        file: "replication/src/vr/transfer.rs",
        original: "        if m.view != self.view || self.status == Status::ViewChange || self.asked != Some(m.op) {",
        replacement: "        if m.view != self.view || self.status == Status::ViewChange {",
        integration: &[TRANSFER_SWEEP],
        unit: &[],
    },
    Mutation {
        name: "vr-retain-past-commit",
        package: Package::Replication,
        breaks: "VR: retention lets records go past the commit-number",
        file: "replication/src/vr.rs",
        original: "        let through = through.min(self.commit);",
        replacement: "        let through = through.max(self.commit);",
        integration: &["retention::a_replica_lets_committed_records_go_and_fetches_state_when_behind"],
        unit: &[],
    },
    Mutation {
        name: "vr-recovering-takes-messages",
        package: Package::Replication,
        breaks: "VR: a replica recovering takes every message but its recovery's",
        file: "replication/src/vr.rs",
        original: "        if self.status == Status::Recovering && !of_recovery {",
        replacement: "        if self.status == Status::Recovering && of_recovery {",
        integration: &[LOST_JOURNAL, BOOTSTRAP],
        unit: &[],
    },
    Mutation {
        name: "vr-recovery-nonce-ignored",
        package: Package::Replication,
        breaks: "VR: a RECOVERYRESPONSE of another nonce counts",
        file: "replication/src/vr/recovery.rs",
        original: "        if self.status != Status::Recovering || m.nonce != self.recovering.nonce {",
        replacement: "        if self.status != Status::Recovering {",
        integration: &[LOST_JOURNAL],
        unit: &[],
    },
    Mutation {
        name: "vr-bootstrap-alone",
        package: Package::Replication,
        breaks: "VR: a fresh replica bootstraps a group one fresh member short",
        file: "replication/src/vr/recovery.rs",
        original: "&& fresh >= f &&",
        replacement: "&& fresh.saturating_add(1) >= f &&",
        integration: &[BOOTSTRAP],
        unit: &[],
    },
    Mutation {
        name: "transfer-export-change-ignored",
        package: Package::Replication,
        breaks: "state transfer: a piece of another export than the fetch began with is taken",
        file: "replication/src/transfer.rs",
        original: "            PIECE_OF if self.image.is_some() && self.export != Some(export) => self.restart()?,",
        replacement: "            PIECE_OF if self.image.is_some() && self.export != Some(export) && false => self.restart()?,",
        integration: &["transfer::whole_files_cross_a_request_at_a_time"],
        unit: &[],
    },
];
