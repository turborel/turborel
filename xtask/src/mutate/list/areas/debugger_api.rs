//! The mutations of the routine debugger's protocol, client and
//! command-line client (RULES.md R-TEST-9, D1030 to D1036): its
//! capability, the attach rule, the connection's ID, the events sent and
//! ended, the events a client keeps and the condition `\debug break`
//! takes.

use super::super::{Mutation, Package};

/// The file of the server's debugger's requests.
const DEBUGGED: &str = "server/src/native/requests/debugged.rs";

/// Mechanisms whose removal must fail the debugger's protocol's
/// requirements.
pub const DEBUGGER_API: [Mutation; 8] = [
    Mutation {
        name: "debug-ungranted-taken",
        package: Package::Server,
        breaks: "a Debug request is served on a connection not granted `debugger`",
        file: "server/src/native/next.rs",
        original: "        Request::Debug(_) => one(DEBUGGER),",
        replacement: "        Request::Debug(_) => [None, None],",
        integration: &["debugging::a_debug_request_needs_the_capability_granted"],
        unit: &[],
    },
    Mutation {
        name: "debug-other-users-session",
        package: Package::Server,
        breaks: "a debugger attaches to the session of a connection of any user",
        file: DEBUGGED,
        original: "        if !mine && !self.session.may_monitor() {",
        replacement: "        if !mine && !self.session.may_monitor() && mine {",
        integration: &["debugging::a_debugger_attaches_only_where_it_may"],
        unit: &[],
    },
    Mutation {
        name: "debug-identify-zero",
        package: Package::Server,
        breaks: "a connection is told no ID of its own",
        file: DEBUGGED,
        original: "                let id = self.serves.present.as_ref().map_or(0, crate::shown::Present::id);",
        replacement: "                let id = 0;",
        integration: &["debugging::a_connection_is_told_its_id"],
        unit: &[],
    },
    Mutation {
        name: "debug-event-dropped",
        package: Package::Server,
        breaks: "a debugger is sent none of its target's events",
        file: "server/src/native/debugging.rs",
        original: "    services.idle(wire.send_now(&Reply::Event(event))).await?;",
        replacement: "    drop(event);",
        integration: &["debugging::a_debugger_drives_another_connections_procedure"],
        unit: &[],
    },
    Mutation {
        name: "debug-ended-kept",
        package: Package::Server,
        breaks: "a debugger whose target ended stays attached to it",
        file: "server/src/native/debugging.rs",
        original: "            held.debugging = None;\n            Event::Ended",
        replacement: "            Event::Ended",
        integration: &["debugging::a_debugger_hears_a_timeout_and_its_targets_end"],
        unit: &[],
    },
    Mutation {
        name: "debug-event-ends-request",
        package: Package::Client,
        breaks: "an event read as a request's last reply ends the request it came within",
        file: "protocol/src/reply.rs",
        original: "            DESCRIPTION | ROWS | NOTICE | STATUS | EVENT => false,",
        replacement: "            DESCRIPTION | ROWS | NOTICE | STATUS => false,",
        integration: &[],
        unit: &["frames::tests::an_event_read_past_ends_no_request"],
    },
    Mutation {
        name: "debug-events-not-kept",
        package: Package::Client,
        breaks: "an event read while a reply is awaited is lost",
        file: "client/src/frames.rs",
        original: "            Reply::Event(event) => self.told.events.push_back(event),",
        replacement: "            Reply::Event(event) => drop(event),",
        integration: &[],
        unit: &["frames::tests::an_event_is_kept_until_waited_for"],
    },
    Mutation {
        name: "debug-condition-cut",
        package: Package::Cli,
        breaks: "`\\debug break` takes its condition from its line's number on",
        file: "cli/src/command/debug.rs",
        original: "const BEFORE_CONDITION: usize = 4;",
        replacement: "const BEFORE_CONDITION: usize = 3;",
        integration: &[],
        unit: &["command::tests::debug_reads_its_words"],
    },
];
