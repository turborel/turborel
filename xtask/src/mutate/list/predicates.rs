//! Mutations of named predicate probes: full rechecks and the order of
//! nullable candidates (RULES.md D34, R-TEST-9).

use super::{Mutation, Package};

/// The named predicate mechanisms a wrong result exposes.
pub const PREDICATES: [Mutation; 2] = [
    Mutation {
        name: "predicate-recheck",
        package: Package::Core,
        breaks: "a named predicate's extra candidates pass without the full condition rechecked",
        file: "src/physical/access.rs",
        original: "            self.used.clear();",
        replacement: "            self.used.truncate(usize::MAX);",
        integration: &["predicate_indexes::candidates_are_rechecked_against_the_whole_condition"],
        unit: &[],
    },
    Mutation {
        name: "predicate-null-order",
        package: Package::Core,
        breaks: "a named predicate's nullable candidates claim a key order that is not SQL's null order",
        file: "src/physical/access.rs",
        original: " && !predicate::untested_nulls(&columns, c.probes.first())",
        replacement: "",
        integration: &["predicate_indexes::nullable_candidates_keep_the_requested_null_order"],
        unit: &[],
    },
];
