//! The mutations of what keeps a server's requests off each other's cache
//! lines (RULES.md R-TEST-9, D815): a worker's permit kept while none
//! waits, the server's stage read from its byte, and whether a
//! connection's user is one asked again once a snapshot is published.

use super::super::{Mutation, Package};

/// The server's lines' mutations, in the order they run.
pub const SERVER_LINES: [Mutation; 3] = [
    Mutation {
        name: "permit-lent-always-given-up",
        package: Package::Server,
        breaks: "every request takes the workers' limit twice, a line every worker writes",
        file: "server/src/limit.rs",
        original: "        if self.workers.wanted() {\n            self.release();",
        replacement: "        if true {\n            self.release();",
        integration: &[],
        unit: &["limit::tests::a_permit_lent_is_kept_while_none_waits"],
    },
    Mutation {
        name: "stage-byte-unwritten",
        package: Package::Server,
        breaks: "a server told to stop still read as serving by its workers",
        file: "server/src/server/stage.rs",
        original: "        self.now.0.store(stage.byte(), Ordering::SeqCst);\n",
        replacement: "",
        integration: &[],
        unit: &["server::stage::tests::the_stage_is_read_as_told"],
    },
    Mutation {
        name: "user-never-asked-again",
        package: Package::Server,
        breaks: "a dropped user's connection served on, its user asked only once",
        file: "server/src/native/lent.rs",
        original: "        if seen >> 1_u32 == count.saturating_add(1) {",
        replacement: "        if seen != 0 {",
        integration: &["ending::a_dropped_users_sessions_end"],
        unit: &[],
    },
];
