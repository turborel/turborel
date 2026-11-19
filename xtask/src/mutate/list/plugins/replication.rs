//! The mutations of replication's and the archive's host kinds
//! (RULES.md R-TEST-9, D270 to D274): a part's failure kept, a journal's
//! naming checked, a transfer's chunks checked and its staging cleared, a
//! consensus's failure halting the driver, a transport's frames taken, the
//! node stopped, a host archive's errors and its scheme, and the SDK's
//! digest.

use super::super::{Mutation, Package};

/// The mutations of the replication and archive host kinds.
pub const REPLICATION_PLUGINS: [Mutation; 10] = [
    Mutation {
        name: "repl-plugin-failure-forgotten",
        package: Package::Replication,
        breaks: "a replication plug-in's part calls its program again after it failed",
        file: "replication/src/plugin.rs",
        original: "        if let Some(e) = self.failed() {\n            return Err(e);\n        }\n        let got",
        replacement: "        let got",
        integration: &[],
        unit: &["plugin::tests::a_plugin_transport_fails_stop"],
    },
    Mutation {
        name: "repl-journal-names-unpersisted",
        package: Package::Replication,
        breaks: "a plug-in journal's naming of a record it was not given is acknowledged",
        file: "replication/src/plugin/journal.rs",
        original: "if self.waiting.front() != Some(&at) {",
        replacement: "if self.waiting.front() == Some(&u64::MAX) {",
        integration: &[],
        unit: &["plugin::tests::a_plugin_journal_acknowledges_only_what_it_was_given"],
    },
    Mutation {
        name: "repl-transfer-digest-unchecked",
        package: Package::Replication,
        breaks: "a plug-in transfer stages a chunk not of its digest",
        file: "replication/src/plugin/transfer.rs",
        original: "if chunk.is_empty() || chunk_digest(&chunk) != digest {",
        replacement: "if chunk.is_empty() {",
        integration: &[],
        unit: &["plugin::tests::a_plugin_transfer_stages_only_checked_chunks"],
    },
    Mutation {
        name: "repl-transfer-staging-kept",
        package: Package::Replication,
        breaks: "a plug-in transfer keeps what an earlier fetch half staged",
        file: "replication/src/plugin/transfer.rs",
        original: "        clear(&*self.fs, &self.dir)?;",
        replacement: "        self.fs.create_dir_all(&self.dir)?;",
        integration: &[],
        unit: &["plugin::tests::a_plugin_transfer_stages_only_checked_chunks"],
    },
    Mutation {
        name: "repl-consensus-failure-ignored",
        package: Package::Replication,
        breaks: "the driver goes on after a plug-in consensus failed mid-step",
        file: "replication/src/driver.rs",
        original: "if let Some(e) = self.consensus.failed() {",
        replacement: "if let Some(e) = None::<SqlError> {",
        integration: &["plugged::a_plugin_consensus_killed_mid_view_change_is_remade_and_agrees"],
        unit: &[],
    },
    Mutation {
        name: "repl-pump-skipped",
        package: Package::Replication,
        breaks: "a plug-in transport's arrived frames are never taken",
        file: "replication/src/driver.rs",
        original: "if let Err(e) = pump() {",
        replacement: "if let Err(e) = Ok::<(), SqlError>(()) {",
        integration: &["plugged::a_group_served_by_plugins_converges"],
        unit: &[],
    },
    Mutation {
        name: "repl-halt-untold",
        package: Package::Replication,
        breaks: "a driver halted by a plug-in's death does not stop its node",
        file: "replication/src/driver.rs",
        original: "            tell(e);\n",
        replacement: "            drop(tell);\n",
        integration: &["plugged::real_databases_stop_their_node_as_a_plugin_dies"],
        unit: &[],
    },
    Mutation {
        name: "archive-error-stops-node",
        package: Package::Core,
        breaks: "a host archive's own ERROR stops the node",
        file: "src/plugin/bridge/archive.rs",
        original: "let process = e.state.is_some_and(|s| [ENDED, TIMED_OUT, VIOLATION, TOO_LONG].contains(&s));",
        replacement: "let process = e.state.is_some();",
        integration: &["plugin_archive::a_host_archive_serves_a_backup_and_its_restore"],
        unit: &[],
    },
    Mutation {
        name: "archive-not-consulted",
        package: Package::Core,
        breaks: "a location of a host archive's scheme is not reached through it",
        file: "src/db/backup.rs",
        original: "if let Some(archive) = self.host_archive(scheme)? {",
        replacement: "if let Some(archive) = None::<PluginTargets> {",
        integration: &["plugin_archive::a_host_archive_serves_a_backup_and_its_restore"],
        unit: &[],
    },
    Mutation {
        name: "sdk-chunk-digest-missing",
        package: Package::Plugin,
        breaks: "the SDK sends a transfer's chunk without its digest",
        file: "plugin/src/host/replication/transfer.rs",
        original: "out.put_u64(p::chunk_digest(&chunk));",
        replacement: "out.put_u64(0);",
        integration: &[],
        unit: &["host::replication::tests::a_transfer_gives_chunks_with_their_digest"],
    },
];
