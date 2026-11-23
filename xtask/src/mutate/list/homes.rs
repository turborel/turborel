//! The mutations of a connection's home (RULES.md R-TEST-9, D767-D769,
//! D842): its requests run on one thread, a home that retired, failed or
//! was closed takes no request and gives its place back, and a home waits
//! on a client that thinks, its permit given up, the idle time counted
//! from the last reply.

use super::{Mutation, Package};

/// The homes' mutations, in the order they run.
pub const HOMES: [Mutation; 8] = [
    Mutation {
        name: "home-never-opened",
        package: Package::Server,
        breaks: "a connection's requests each handed to any free thread of the pool, its session moving between threads",
        file: "server/src/native/served.rs",
        original: "    *home = services.hot().map(|place| Home::open(place, KEPT));",
        replacement: "    *home = None;",
        integration: &["homes::a_connections_statements_run_on_one_thread", "homes::connections_past_the_workers_wait_their_turn_at_home"],
        unit: &[],
    },
    Mutation {
        name: "home-takes-requests-retired",
        package: Package::Server,
        breaks: "a request handed to a home whose thread has gone, never run",
        file: "server/src/native/home.rs",
        original: "        if state.retired {\n            return Err(job);",
        replacement: "        if false {\n            return Err(job);",
        integration: &[],
        unit: &["native::home::tests::a_home_retires_once_idle_or_closed", "native::home::tests::a_home_that_failed_takes_no_job"],
    },
    Mutation {
        name: "home-failed-not-retired",
        package: Package::Server,
        breaks: "a home whose thread panicked still taking requests",
        file: "server/src/native/home.rs",
        original: "        let mut state = self.shared.locked();\n        state.retired = true;\n        // A job handed over",
        replacement: "        let mut state = self.shared.locked();\n        // A job handed over",
        integration: &[],
        unit: &["native::home::tests::a_home_that_failed_takes_no_job"],
    },
    Mutation {
        name: "home-closed-kept",
        package: Package::Server,
        breaks: "a home kept, with its thread and place, after its connection has gone",
        file: "server/src/native/home.rs",
        original: "        self.shared.locked().closed = true;",
        replacement: "        drop(self.shared.locked());",
        integration: &[],
        unit: &["native::home::tests::a_home_retires_once_idle_or_closed"],
    },
    Mutation {
        name: "home-waits-only-on-fast-clients",
        package: Package::Server,
        breaks: "a client that thinks past a millisecond read by the connection's task, two moves between threads a request",
        file: "server/src/native/lent.rs",
        original: "        (self.fast || self.homed) && self.direct",
        replacement: "        self.fast && self.direct",
        integration: &[],
        unit: &["native::lent::tests::a_worker_waits_on_a_fast_client_read_directly"],
    },
    Mutation {
        name: "home-wait-deaf-to-the-end",
        package: Package::Server,
        breaks: "a home waiting on its client blind to its operator's end and the server stopping",
        file: "server/src/native/waiting.rs",
        original: "        if !self.homed || leaving() {",
        replacement: "        if !self.homed {",
        integration: &[],
        unit: &["native::waiting::tests::a_home_waits_on_in_slices_and_another_worker_does_not"],
    },
    Mutation {
        name: "home-wait-keeps-its-permit",
        package: Package::Server,
        breaks: "a home waiting on a slow client keeps its permit, a statement of another connection waiting on it",
        file: "server/src/native/waiting.rs",
        original: "        self.began.elapsed() >= WAITED",
        replacement: "        self.began.elapsed() >= WAITED.saturating_mul(1000)",
        integration: &[],
        unit: &["native::waiting::tests::a_wait_is_slow_once_waited_has_passed"],
    },
    Mutation {
        name: "idle-counted-anew-from-the-hand-back",
        package: Package::Server,
        breaks: "a client's idle time counted anew as its home gives it back, so it idles past the limit",
        file: "server/src/native/next.rs",
        original: "        let since = lent.idle_since.take().unwrap_or(began);",
        replacement: "        let since = began;",
        integration: &["waiting::a_client_idle_past_the_limit_is_closed_though_its_home_waited"],
        unit: &[],
    },
];
