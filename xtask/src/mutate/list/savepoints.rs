//! The mutations of savepoints (RULES.md R-TEST-9, D88): the limit let
//! past, RELEASE keeping the savepoint it names, a savepoint of another
//! level or before an atomic context named, cursors left open and later
//! locators left valid, an engine joined after a savepoint not gone back,
//! staged definitions left uncancelled, and recovery replaying them.

use super::{Mutation, Package};

/// The mutations of savepoints, in the order they run.
pub const SAVEPOINTS: [Mutation; 9] = [
    Mutation {
        name: "savepoint-limit-passed",
        package: Package::Core,
        breaks: "a transaction takes a 1,025th savepoint",
        file: "src/db/savepoints.rs",
        original: "if self.named.len() >= MAX_SAVEPOINTS {",
        replacement: "if self.named.len() > MAX_SAVEPOINTS {",
        integration: &["savepoints::at_most_1024_savepoints_17_5"],
        unit: &["db::savepoints::tests::savepoints_are_at_most_the_limit_17_5"],
    },
    Mutation {
        name: "savepoint-release-keeps-it",
        package: Package::Core,
        breaks: "RELEASE SAVEPOINT destroys those after the savepoint but not the savepoint",
        file: "src/db/savepoints.rs",
        original: "Ok(self.named.split_off(at).into_iter().map(|e| e.held).collect())",
        replacement: "Ok(self.named.split_off(at.saturating_add(1)).into_iter().map(|e| e.held).collect())",
        integration: &["savepoints::release_destroys_it_and_those_after_17_6", "savepoints::savepoints_nest_and_a_name_is_replaced_17_5"],
        unit: &[],
    },
    Mutation {
        name: "savepoint-level-unchecked",
        package: Package::Core,
        breaks: "a statement names a savepoint of another savepoint level",
        file: "src/db/savepoints.rs",
        original: "self.named.iter().position(|e| e.level == level && e.name == name)",
        replacement: "self.named.iter().rposition(|e| e.level <= level && e.name == name)",
        integration: &["savepoints::levels::a_new_level_procedure_has_its_own_savepoints_9_18"],
        unit: &[],
    },
    Mutation {
        name: "savepoint-atomic-context-crossed",
        package: Package::Core,
        breaks: "ROLLBACK TO goes back past the beginning of the atomic execution context running",
        file: "src/db/savepoints.rs",
        original: "if entry.atomic < self.atomic() {",
        replacement: "if entry.atomic > self.atomic() {",
        integration: &["savepoints::levels::an_atomic_block_runs_at_a_level_of_its_own_14_1"],
        unit: &[],
    },
    Mutation {
        name: "savepoint-cursors-open",
        package: Package::Core,
        breaks: "ROLLBACK TO SAVEPOINT leaves cursors open",
        file: "src/db/savepoints.rs",
        original: "self.close_cursors();",
        replacement: "let _ = Self::close_cursors;",
        integration: &["savepoints::rollback_to_closes_cursors_and_invalidates_later_locators_17_8"],
        unit: &[],
    },
    Mutation {
        name: "savepoint-locators-valid",
        package: Package::Core,
        breaks: "ROLLBACK TO SAVEPOINT leaves valid the locators given after the savepoint",
        file: "src/db/savepoints.rs",
        original: "self.locators.borrow_mut().given_after(locators);",
        replacement: "self.locators.borrow_mut().given_after(locators.saturating_add(1));",
        integration: &["savepoints::rollback_to_closes_cursors_and_invalidates_later_locators_17_8"],
        unit: &[],
    },
    Mutation {
        name: "savepoint-late-engine-kept",
        package: Package::Core,
        breaks: "an engine that undoes its own changes and joins after a savepoint is not gone back",
        file: "src/txn/access.rs",
        original: "tx.joined_after_savepoints(joined);",
        replacement: "let _ = joined;",
        integration: &["savepoints::savepoints_nest_and_a_name_is_replaced_17_5", "savepoints::rollback_to_keeps_the_savepoint_17_8"],
        unit: &[],
    },
    Mutation {
        name: "savepoint-definitions-kept",
        package: Package::Core,
        breaks: "ROLLBACK TO SAVEPOINT keeps the changes of definitions staged after the savepoint",
        file: "src/db/savepoints.rs",
        original: "let cancelled = if now > staged { self.cancel_definitions(cx.tx, staged) } else { Ok(()) };",
        replacement: "let cancelled: Result<()> = Ok(());",
        integration: &[
            "savepoints::durability::recovery_leaves_out_definitions_cancelled_by_a_savepoint",
            "savepoints::levels::a_body_cancels_no_definition_its_caller_runs_on",
        ],
        unit: &[],
    },
    Mutation {
        name: "savepoint-recovery-replays-cancelled",
        package: Package::Core,
        breaks: "recovery and a standby make the definitions a rollback to a savepoint cancelled",
        file: "src/db/recovery.rs",
        original: "replaying.cancel(tx, from);",
        replacement: "let _ = (tx, from);",
        integration: &[
            "savepoints::durability::recovery_leaves_out_definitions_cancelled_by_a_savepoint",
            "savepoints::durability::a_standby_leaves_out_definitions_cancelled_by_a_savepoint",
        ],
        unit: &[],
    },
];
