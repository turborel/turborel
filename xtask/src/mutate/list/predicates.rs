//! Break the WHERE check or SQL null ordering after a function lookup,
//! and require the result tests to catch it (RULES.md D34, R-TEST-9).

use super::{Mutation, Package};

/// Deliberate faults that must produce wrong rows or wrong order in tests.
pub const PREDICATES: [Mutation; 2] = [
    Mutation {
        name: "predicate-recheck",
        package: Package::Core,
        breaks: "a function lookup returns extra rows because the full WHERE check is removed",
        file: "src/physical/access.rs",
        original: "            self.used.clear();",
        replacement: "            self.used.truncate(usize::MAX);",
        integration: &[
            "predicate_indexes::candidates_are_rechecked_against_the_whole_condition",
            "geometry::index::geometry_index_filters_refine_candidates_and_equal_scans",
        ],
        unit: &[],
    },
    Mutation {
        name: "predicate-null-order",
        package: Package::Core,
        breaks: "a function lookup skips the sort needed to put surviving nulls in SQL order",
        file: "src/physical/access.rs",
        original: " && !predicate::untested_nulls(on.columns, c.probes.first())",
        replacement: "",
        integration: &["predicate_indexes::nullable_candidates_keep_the_requested_null_order"],
        unit: &[],
    },
];
