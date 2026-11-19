//! The mutations of host plug-ins (RULES.md R-TEST-9, D229, D246 to
//! D254): the node stopped by a failure and its statements refused, the
//! pin of package cryptography, a manifest of both sorts, the directory
//! the host names, each page encoded through the program, a commit's
//! records sent before it is durable, the files a database is opened
//! from, a run's records sent, and the listener.

use super::super::{Mutation, Package};

/// The mutations of host plug-ins.
pub const HOSTED: [Mutation; 10] = [
    Mutation {
        name: "host-failure-goes-on",
        package: Package::Core,
        breaks: "a host plug-in's failed call leaves the node running",
        file: "src/plugin/hosted.rs",
        original: "            self.plugin.stop.stop(&why);\n",
        replacement: "            let _ = &why;\n",
        integration: &["host_plugins::a_host_plugins_failure_stops_the_node"],
        unit: &[],
    },
    Mutation {
        name: "host-stop-unread",
        package: Package::Core,
        breaks: "statements run on after a host plug-in's failure stopped the node",
        file: "src/db/hosted.rs",
        original: "    if !st.extension_host.stop.tripped() {",
        replacement: "    if true {",
        integration: &["host_plugins::a_host_plugins_failure_stops_the_node"],
        unit: &[],
    },
    Mutation {
        name: "host-crypto-unpinned",
        package: Package::Core,
        breaks: "package cryptography starts with no digest pinned",
        file: "src/plugin/hosted.rs",
        original: "    if crypto_kind && spec.digest.is_none() {",
        replacement: "    if false {",
        integration: &["host_plugins::host_plugins_are_declared_by_the_host_alone"],
        unit: &[],
    },
    Mutation {
        name: "host-manifest-mixed",
        package: Package::Core,
        breaks: "a manifest declares host objects and an extension's at once",
        file: "src/plugin/manifest.rs",
        original: "        if !self.hosted.is_empty() && !self.objects.is_empty() {",
        replacement: "        if false {",
        integration: &["host_plugins::host_plugins_are_declared_by_the_host_alone"],
        unit: &[],
    },
    Mutation {
        name: "host-directory-unnamed",
        package: Package::Core,
        breaks: "a host plug-in's directory must be under PLUGINS.DIRECTORIES too",
        file: "src/plugin/pool.rs",
        original: "let directory = if self.named {",
        replacement: "let directory = if false {",
        integration: &["host_plugins::a_host_plugins_objects_are_the_hosts_own"],
        unit: &[],
    },
    Mutation {
        name: "host-encoding-skipped",
        package: Package::Core,
        breaks: "a page is written as it is, not as the host encoding gives it",
        file: "src/plugin/bridge/hosted/page.rs",
        original: "        let got = self.link.call(ENCODING_ENCODE, payload)?;\n        out.extend_from_slice(&answer(&got, |r| r.bytes().map(<[u8]>::to_vec))?);\n",
        replacement: "        drop(self.link.call(ENCODING_ENCODE, payload)?);\n        out.extend_from_slice(input);\n",
        integration: &["host_plugins::pages::pages_pass_through_a_host_plugins_encoding"],
        unit: &[],
    },
    Mutation {
        name: "host-log-sync-unsent",
        package: Package::Core,
        breaks: "a commit is taken as durable without its records sent to the host log",
        file: "src/plugin/bridge/hosted/store.rs",
        original: "            let done = self.send(&batch);",
        replacement: "            let done: Result<()> = Ok(());",
        integration: &["host_plugins::stores::a_host_plugins_log_survives_kills_mid_commit_and_mid_recovery"],
        unit: &[],
    },
    Mutation {
        name: "host-file-system-unused",
        package: Package::Core,
        breaks: "a database is opened from the system's files though a host file system is started",
        file: "src/db/hosted.rs",
        original: "            Some((_, link)) => Arc::new(PluginFiles::new(link)),",
        replacement: "            Some(_) => Arc::new(OsFileSystem),",
        integration: &["host_plugins::stores::a_host_plugins_file_system_survives_a_kill_mid_page_write"],
        unit: &[],
    },
    Mutation {
        name: "host-spill-run-unsent",
        package: Package::Core,
        breaks: "a record pushed onto a host spill run is never sent",
        file: "src/plugin/bridge/hosted/spill.rs",
        original: "self.0.add(SPILL_PUSH, &[record])",
        replacement: "self.0.add(SPILL_PUSH, &[])",
        integration: &["host_plugins::stores::a_sort_spills_to_a_host_plugin"],
        unit: &[],
    },
    Mutation {
        name: "host-listener-unbound",
        package: Package::Server,
        breaks: "a server listens nowhere a host plug-in's listener serves",
        file: "server/src/listening.rs",
        original: "    for (_, link) in db.host_objects(HostKind::Listener)? {",
        replacement: "    for (_, link) in db.host_objects(HostKind::Listener)?.into_iter().take(0) {",
        integration: &["host_plugins::a_client_is_served_through_a_host_plugins_listener"],
        unit: &[],
    },
];
