//! The mutations of a statement's fixed cost (RULES.md R-TEST-9): the
//! unnamed cursors a session keeps track of, the state overlaid with its
//! own settings, and the tables of the INSERTs it runs directly, kept
//! between statements; and of a prepared CALL run straight (D940), the
//! binding it is kept beside, its arguments' casts, its output values and
//! its savepoint level, the values read for locators
//! (D941), and the diagnostics area of a body that reads it (D943).

use super::{Mutation, Package};

/// The mutations of a statement's fixed cost, in the order they run.
pub const CALL_FLOOR: [Mutation; 11] = [
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
    Mutation {
        name: "call-direct-any-binding",
        package: Package::Core,
        breaks: "a prepared CALL bound again runs the program kept beside another binding, the body of before",
        file: "src/db/routines/direct.rs",
        original: "        let kept = inner.routines.1.0.iter_mut().find(|k| Arc::ptr_eq(&k.bound, bound))?;\n",
        replacement: "        let kept = inner.routines.1.0.iter_mut().find(|k| k.running.is_some())?;\n",
        integration: &[],
        unit: &["db::routines::direct::tests::a_binding_bound_again_finds_nothing_kept"],
    },
    Mutation {
        name: "call-direct-uncast-argument",
        package: Package::Core,
        breaks: "an argument of a CALL run straight is not cast to its parameter's type, and one cast with a warning is given",
        file: "src/db/routines/call.rs",
        original: "                let (value, warning) = conformed(slots.get(*index)?.clone(), to).ok()?;\n",
        replacement: "                let (value, warning) = (slots.get(*index)?.clone(), None::<crate::error::SqlState>);\n",
        integration: &[],
        unit: &["db::routines::direct::tests::a_value_cast_to_a_settled_type_is_assigned_unchanged"],
    },
    Mutation {
        name: "call-direct-outputs-lost",
        package: Package::Core,
        breaks: "a CALL run straight gives back no output parameter's value",
        file: "src/db/routines/direct.rs",
        original: "            bound.dynamic.iter().try_for_each(|(k, index)| params.set_dynamic(*index, out.get(*k).cloned().unwrap_or(Value::Null)))\n",
        replacement: "            bound.dynamic.iter().try_for_each(|(k, index)| params.set_dynamic(*index, out.get(usize::MAX - *k).cloned().unwrap_or(Value::Null)))\n",
        integration: &[],
        unit: &["db::routines::direct::tests::a_prepared_call_runs_straight_as_any_call_runs"],
    },
    Mutation {
        name: "call-direct-callers-level",
        package: Package::Core,
        breaks: "a body of NEW SAVEPOINT LEVEL run straight runs at its caller's savepoint level",
        file: "src/db/routines/direct.rs",
        original: "            let level = running.level.then_some(Level::Procedure);\n",
        replacement: "            let level = running.level.then_some(Level::Procedure).filter(|_| false);\n",
        integration: &[],
        unit: &["db::routines::direct::tests::atomic_bodies_and_savepoint_levels_are_kept"],
    },
    Mutation {
        name: "prepared-locator-unread",
        package: Package::Core,
        breaks: "a large object an execution is given by its locator is not read",
        file: "src/db/prepared/locators.rs",
        original: "        Value::Lob(_) | Value::Structured(_) => true,\n",
        replacement: "        Value::Lob(_) | Value::Structured(_) => false,\n",
        integration: &[],
        unit: &["db::prepared::locators::tests::only_a_large_object_or_what_holds_one_may_hold_a_locator"],
    },
    Mutation {
        name: "compiled-area-unread",
        package: Package::Core,
        breaks: "a compiled body with a GET DIAGNOSTICS places no statement's condition, so it reads the CALL's own",
        file: "src/db/routines/program/compiled/run.rs",
        original: "        Code::Statement(s) => rows_alone(s.contained.statement()),\n",
        replacement: "        Code::Statement(_) => true,\n",
        integration: &["compiled_procedures::a_body_that_reads_no_diagnostics_gives_the_same"],
        unit: &[],
    },
];
