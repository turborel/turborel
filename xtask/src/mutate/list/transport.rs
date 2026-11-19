//! The mutations of the transport (RULES.md R-TEST-9, D230-D235): a
//! request's last replies held for the next pipelined request's only
//! within a batch, the reply buffer begun anew once sent, a stream TLS
//! secures read by its worker itself and failed by a socket closed without
//! close_notify, and a request pipelined behind a stop not run.

use super::{Mutation, Package};

/// The transport's mutations, in the order they run.
pub const TRANSPORT: [Mutation; 5] = [
    Mutation {
        name: "pipelined-replies-past-a-batch",
        package: Package::Server,
        breaks: "a pipelined request's replies held past SERVER.BATCH_BYTES",
        file: "server/src/native/lent.rs",
        original: "        self.whole && self.room",
        replacement: "        self.whole",
        integration: &[],
        unit: &["native::lent::tests::replies_wait_only_for_a_request_come_whole_within_a_batch"],
    },
    Mutation {
        name: "reply-buffer-never-begun-anew",
        package: Package::Server,
        breaks: "the reply buffer keeps every reply sent, growing with the connection's life",
        file: "server/src/native/wire.rs",
        original: "        if self.sent == self.out.len() {",
        replacement: "        if false {",
        integration: &[],
        unit: &["native::wire::tests::replies_queue_into_one_write_and_requests_are_seen_whole"],
    },
    Mutation {
        name: "tls-read-by-the-runtime-alone",
        package: Package::Server,
        breaks: "a stream TLS secures never read by its worker itself, each request a hop between threads",
        file: "server/src/tls/direct.rs",
        original: "        self.0.get_mut().0.direct()?;\n        Some(self)",
        replacement: "        self.0.get_mut().0.direct()?;\n        None",
        integration: &[],
        unit: &["tls::direct::tests::a_tls_stream_is_read_and_written_by_a_thread_itself"],
    },
    Mutation {
        name: "tls-cut-taken-for-an-end",
        package: Package::Server,
        breaks: "a TLS socket closed without close_notify taken as the client ending its stream",
        file: "server/src/tls/direct.rs",
        original: "                return session.reader().read(buf);",
        replacement: "                return Ok(0);",
        integration: &[],
        unit: &["tls::direct::tests::a_socket_closed_without_close_notify_fails_the_read"],
    },
    Mutation {
        name: "pipelined-request-run-past-a-stop",
        package: Package::Server,
        breaks: "a request pipelined behind the one in flight as the server stops is run",
        file: "server/src/native/lent.rs",
        original: "super::next::work_of(request, granted).filter(|_| running.still()))",
        replacement: "super::next::work_of(request, granted))",
        integration: &["transport::a_server_stopping_sends_the_replies_in_flight"],
        unit: &[],
    },
];
