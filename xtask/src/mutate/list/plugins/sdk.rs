//! The mutations of the plug-in SDK, `turborel-protocol`'s `plugin`
//! (RULES.md R-TEST-9, D167 to D170): the frames `serve_on` answers, the
//! limits HELLO gives, a batch answered row by row, cancellation, the
//! answer's limit and an error's SQLSTATE.

use super::super::{Mutation, Package};

/// The mutations of the SDK.
pub const SDK: [Mutation; 7] = [
    Mutation {
        name: "sdk-welcome-takes-nothing",
        package: Package::Protocol,
        breaks: "WELCOME takes none of the capabilities HELLO offers",
        file: "protocol/src/plugin/serve.rs",
        original: ".filter(|c| taken.contains(&c.as_str()))",
        replacement: ".filter(|c| c.is_empty())",
        integration: &[],
        unit: &["plugin::tests::frames_are_served"],
    },
    Mutation {
        name: "sdk-hello-limits-ignored",
        package: Package::Protocol,
        breaks: "the limit of a message HELLO gives is not kept",
        file: "protocol/src/plugin/serve.rs",
        original: "limits = Limits { max_message_bytes: h.max_message_bytes,",
        replacement: "limits = Limits { max_message_bytes: 0,",
        integration: &[],
        unit: &["plugin::tests::frames_are_served"],
    },
    Mutation {
        name: "sdk-shutdown-ignored",
        package: Package::Protocol,
        breaks: "a plug-in goes on serving past SHUTDOWN",
        file: "protocol/src/plugin/serve.rs",
        original: "Message::Shutdown => return Ok(()),",
        replacement: "Message::Shutdown => {}",
        integration: &[],
        unit: &["plugin::tests::frames_are_served"],
    },
    Mutation {
        name: "sdk-batch-first-row",
        package: Package::Protocol,
        breaks: "a scalar's batch is answered for its first row alone",
        file: "protocol/src/plugin/dispatch.rs",
        original: "    for row in &all {\n",
        replacement: "    for row in all.iter().take(1) {\n",
        integration: &[],
        unit: &["plugin::tests::calls_are_answered_by_their_objects"],
    },
    Mutation {
        name: "sdk-cancel-unheard",
        package: Package::Protocol,
        breaks: "a call that checks its context is never told of a CANCEL",
        file: "protocol/src/plugin/serve.rs",
        original: "if self.cancelled() { Err(",
        replacement: "if false { Err(",
        integration: &[],
        unit: &["plugin::tests::cancels_and_limits_are_honoured"],
    },
    Mutation {
        name: "sdk-answer-limit-unchecked",
        package: Package::Protocol,
        breaks: "an answer past the limits HELLO gave is sent, not refused with 38T04",
        file: "protocol/src/plugin/serve.rs",
        original: ".saturating_add(16) > m)",
        replacement: ".saturating_add(16) > u64::MAX)",
        integration: &[],
        unit: &["plugin::tests::cancels_and_limits_are_honoured", "plugin::tests::frames_are_served"],
    },
    Mutation {
        name: "sdk-error-state-lost",
        package: Package::Protocol,
        breaks: "a plug-in's error is sent as 38000, its SQLSTATE lost",
        file: "protocol/src/plugin/serve.rs",
        original: "Err(e) => refused(e.state, e.message),",
        replacement: "Err(e) => refused(\"38000\".to_owned(), e.message),",
        integration: &[],
        unit: &["plugin::tests::calls_are_answered_by_their_objects"],
    },
];
