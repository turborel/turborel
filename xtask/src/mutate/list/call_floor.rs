//! The mutations of a statement's fixed cost (RULES.md R-TEST-9): the
//! unnamed cursors a session keeps track of, the state overlaid with its
//! own settings, and the tables of the INSERTs it runs directly, kept
//! between statements.

use super::{Mutation, Package};

/// The mutations of a statement's fixed cost, in the order they run.
pub const CALL_FLOOR: [Mutation; 5] = [
    Mutation {
        name: "call-floor-closed-cursors-kept",
        package: Package::Core,
        breaks: "a closed unnamed cursor stays tracked, so every statement of a long transaction looks at all it ever opened",
        file: "src/db/unnamed.rs",
        original: "        inner.anonymous.retain(|open| open.strong_count() > 0);\n",
        replacement: "",
        integration: &[],
        unit: &["db::unnamed::tests::closed_cursors_are_forgotten"],
    },
    Mutation {
        name: "call-floor-overlaid-state-stale",
        package: Package::Core,
        breaks: "the state overlaid with a session's settings is kept though another is published, so a definition made since is not seen",
        file: "src/db/settings/own.rs",
        original: "            && std::ptr::eq(base.as_ptr(), Arc::as_ptr(&st))\n",
        replacement: "            && true\n",
        integration: &[],
        unit: &["db::settings::own::tests::a_definition_published_by_another_session_is_seen"],
    },
    Mutation {
        name: "call-floor-kept-target-stale",
        package: Package::Core,
        breaks: "an INSERT's table stays resolved though the definitions, privileges or session it was resolved with changed",
        file: "src/db/write/targets.rs",
        original: "        if !self.key.as_ref().is_some_and(|k| k.holds(st, s)) {\n            self.key = None;\n",
        replacement: "        if self.key.is_none() {\n            self.key = None;\n",
        integration: &[
            "kept_targets::a_table_altered_or_made_anew_is_resolved_again",
            "kept_targets::a_view_made_anew_is_resolved_again",
            "kept_targets::a_privilege_revoked_or_granted_is_checked_again",
            "kept_targets::the_default_schema_and_the_authorization_resolve_it_again",
        ],
        unit: &[],
    },
    Mutation {
        name: "call-floor-body-target-kept",
        package: Package::Core,
        breaks: "an INSERT of a procedure's body or a triggered action is resolved as one run directly, every table seen whole, not as it was defined",
        file: "src/params/transition.rs",
        original: "        std::ptr::eq(self.horizon(), &raw const WHOLE)\n",
        replacement: "        true\n",
        integration: &["kept_targets::a_procedure_or_trigger_insert_is_never_kept"],
        unit: &[],
    },
    Mutation {
        name: "call-floor-clocked-target-kept",
        package: Package::Core,
        breaks: "an INSERT's table resolved through a view reading the statement's instant is kept, so a later INSERT is checked by an instant past",
        file: "src/db/write/targets.rs",
        original: "            if !noted.get() {\n",
        replacement: "            if true {\n",
        integration: &["kept_targets::a_view_reading_the_instant_is_resolved_at_each_insert"],
        unit: &[],
    },
];
