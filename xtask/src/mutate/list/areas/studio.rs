//! The mutations of what a database shows of its sessions and statements
//! (RULES.md R-TEST-9, D520, D521): a session's wait, the role MONITOR's
//! creator, a literal's placeholder, the tally that gives way, and the
//! rows a scan read.

use super::super::{Mutation, Package};

/// Mechanisms of the sessions' and statements' views whose removal must
/// fail their tests.
pub const STUDIO: [Mutation; 5] = [
    Mutation {
        name: "studio-wait-not-shown",
        package: Package::Server,
        breaks: "a wait for a row is not noted, so CONNECTIONS shows the waiting connection waiting on nothing",
        file: "src/txn/row_waits.rs",
        original: "        self.stop.activity().waits_for_lock(what.to_string(), writer);\n",
        replacement: "",
        integration: &["activity::a_connection_shows_the_lock_it_waits_on"],
        unit: &[],
    },
    Mutation {
        name: "studio-monitor-of-anyone",
        package: Package::Core,
        breaks: "a role MONITOR any user created shows every statement's text",
        file: "src/system.rs",
        original: "owner == self.auth || (created && roles.applicable(&[self.auth]).contains(MONITOR))",
        replacement: "owner == self.auth || roles.applicable(&[self.auth]).contains(MONITOR)",
        integration: &["statement_statistics::another_users_statement_text_is_null"],
        unit: &[],
    },
    Mutation {
        name: "studio-literal-kept",
        package: Package::Core,
        breaks: "a literal is kept in a normalized text, so runs differing in their literals are tallied apart",
        file: "src/system/statements/text.rs",
        original: "                (PLACEHOLDER.to_owned(), false, false)",
        replacement: "                (written, false, false)",
        integration: &["statement_statistics::statements_are_tallied_by_their_normalized_text"],
        unit: &["system::statements::text::tests::literals_become_placeholders"],
    },
    Mutation {
        name: "studio-kept-unbounded",
        package: Package::Core,
        breaks: "a new tally past STATEMENTS.KEPT is added, none giving way",
        file: "src/txn/stop/activity/tallies.rs",
        original: "        if self.by.len() >= kept {",
        replacement: "        if false {",
        integration: &["statement_statistics::statements_kept_bounds_and_resets_them"],
        unit: &[],
    },
    Mutation {
        name: "studio-rows-read-lost",
        package: Package::Core,
        breaks: "the rows a scan read are not given to its session's count once it is read through",
        file: "src/txn/stop.rs",
        original: "                    None => self.flush(),",
        replacement: "                    None => {}",
        integration: &["statement_statistics::statements_are_tallied_by_their_normalized_text"],
        unit: &[],
    },
];
