//! The mutations of a client kept in step with its server (RULES.md
//! R-TEST-9, D393, D394). A reply with a value the client cannot take is
//! read past with the rest of its request's, a malformed one losing the
//! connection, a Describe's Description its last reply, a Complete's
//! head noted; and Terminate bounded on its stream, a Notice before its
//! Error read past.

use super::super::{Mutation, Package};

/// The unit tests of a connection's frames.
const BAD_VALUE: &str = "frames::tests::a_bad_value_is_read_past_with_its_statements_replies";
const CURSOR: &str = "frames::tests::a_cursor_or_a_describe_with_a_bad_value_reads_only_its_own";

/// Mechanisms whose removal must fail the client's step with its server.
pub const CLIENT_SYNC: [Mutation; 7] = [
    Mutation {
        name: "client-bad-value-not-drained",
        package: Package::Client,
        breaks: "a reply with a value the client cannot take leaves the rest of its request's replies unread",
        file: "client/src/frames.rs",
        original: "                if answer == Answer::Replies {\n",
        replacement: "                if answer == Answer::One {\n",
        integration: &[],
        unit: &[BAD_VALUE, CURSOR],
    },
    Mutation {
        name: "client-more-ignored",
        package: Package::Client,
        breaks: "a Complete that says more replies follow is taken as its request's last",
        file: "protocol/src/reply.rs",
        original: "            COMPLETE => Reply::completed_head(kind, body).is_none_or(|c| !c.more),\n",
        replacement: "            COMPLETE => true,\n",
        integration: &[],
        unit: &[BAD_VALUE],
    },
    Mutation {
        name: "client-describe-drained",
        package: Package::Client,
        breaks: "a Describe of a cursor whose Description holds a bad value reads the next request's replies as its rows",
        file: "client/src/statements.rs",
        original: "        match self.receive_of(Answer::One)? {\n",
        replacement: "        match self.receive()? {\n",
        integration: &[],
        unit: &[CURSOR],
    },
    Mutation {
        name: "client-bad-complete-not-noted",
        package: Package::Client,
        breaks: "a Complete with a bad condition leaves the session's state as before it",
        file: "client/src/frames.rs",
        original: "            self.noted(Reply::Complete(head));\n",
        replacement: "            drop(head);\n",
        integration: &[],
        unit: &[CURSOR],
    },
    Mutation {
        name: "client-malformed-frame-kept",
        package: Package::Client,
        breaks: "a malformed frame fails its statement but keeps the connection",
        file: "client/src/frames.rs",
        original: "            return Err(self.lost_by(failure.clone()));\n",
        replacement: "            return Err(failure.clone());\n",
        integration: &[],
        unit: &["frames::tests::a_malformed_frame_loses_the_connection"],
    },
    Mutation {
        name: "terminate-unbounded",
        package: Package::Client,
        breaks: "Terminate waits without limit on a stream with no read timeout",
        file: "client/src/connection.rs",
        original: "            bounded(self.stream.get_ref(), Some(self.timeouts.read.unwrap_or(TERMINATE_WITHIN)))?;\n",
        replacement: "            bounded(self.stream.get_ref(), self.timeouts.read)?;\n",
        integration: &[],
        unit: &["frames::tests::terminate_waits_on_a_bounded_stream_at_most_its_limit"],
    },
    Mutation {
        name: "terminate-notice-ends",
        package: Package::Client,
        breaks: "a Notice before Terminate's Error is taken as its end, and the Error lost",
        file: "client/src/connection.rs",
        original: "                Reply::Notice(_) | Reply::Status { .. } | Reply::Pong => {}\n",
        replacement: "                Reply::Notice(_) | Reply::Status { .. } | Reply::Pong => return Ok(()),\n",
        integration: &[],
        unit: &["frames::tests::terminate_reads_past_a_notice_to_its_error"],
    },
];
