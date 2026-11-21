//! The mutations of a statement's fixed cost (RULES.md R-TEST-9): the
//! unnamed cursors a session keeps track of, and the state overlaid with
//! its own settings, kept between statements.

use super::{Mutation, Package};

/// The mutations of a statement's fixed cost, in the order they run.
pub const CALL_FLOOR: [Mutation; 2] = [
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
];
