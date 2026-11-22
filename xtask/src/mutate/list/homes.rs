//! The mutations of a connection's home (RULES.md R-TEST-9, D767-D769):
//! its requests run on one thread, and a home that retired, failed or was
//! closed takes no request and gives its place back.

use super::{Mutation, Package};

/// The homes' mutations, in the order they run.
pub const HOMES: [Mutation; 4] = [
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
];
