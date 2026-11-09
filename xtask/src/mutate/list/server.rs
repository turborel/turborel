//! The mutations of the server (RULES.md R-TEST-9, D42): a stream handed
//! to the thread that waits on it for a fast client's next request
//! (`transport/handed.rs`), and a worker's permit taken leaving room
//! letting the next waiter in (`limit.rs`).

use super::{Mutation, Package};

/// The server's mutations, in the order they run.
pub const SERVER: [Mutation; 2] = [
    Mutation {
        name: "handed-to-thread",
        package: Package::Server,
        breaks: "the socket handed to the thread that waits on it, the runtime's interest given up",
        file: "server/src/transport/handed.rs",
        original: "        self.hand_to_thread()?;",
        replacement: "        self.hand_to_runtime()?;",
        integration: &[],
        unit: &[
            "transport::handed::tests::a_stream_is_handed_to_the_thread_that_waits_and_back_as_the_runtime_reads",
            "transport::handed::tests::a_tcp_stream_is_read_and_written_by_a_thread_itself",
        ],
    },
    Mutation {
        name: "permit-room-untold",
        package: Package::Server,
        breaks: "worker permits: one taken leaving room wakes no waiter, so a limit raised lets in one at a time as permits are given back",
        file: "server/src/limit.rs",
        original: "        if room {",
        replacement: "        if false && room {",
        integration: &[],
        unit: &["limit::tests::one_given_back_lets_in_one_waiter_and_room_left_the_next"],
    },
];
