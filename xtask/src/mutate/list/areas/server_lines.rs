//! The mutations of what keeps a server's requests off each other's cache
//! lines (RULES.md R-TEST-9, D815): a worker's permit kept while none
//! waits, and the server's stage read from its byte.

use super::super::{Mutation, Package};

/// The server's lines' mutations, in the order they run.
pub const SERVER_LINES: [Mutation; 2] = [
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
];
