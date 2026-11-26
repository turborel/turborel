//! The mutations of the edges a class of equalities implies (RULES.md
//! R-TEST-9, D1204): a star taken from a column that is no key, the
//! implied edges joining nothing, a star past the budget kept, the stars
//! offered from the relation of most rows, and the unique index of a
//! deferrable constraint taken for a key (D1205).

use super::super::{Mutation, Package};

/// The stars of the classes.
const MODEL_FILE: &str = "src/physical/enumerate/model.rs";
/// The graph.
const GRAPH_FILE: &str = "src/physical/enumerate/graph.rs";
/// The implied edges and their budget.
const IMPLIED_FILE: &str = "src/physical/enumerate/graph/implied.rs";
/// The unit test of the edge.
const JOINED: &str = "physical::enumerate::tests::a_key_of_a_class_joins_its_other_relations";
/// The unit test of the budget.
const BUDGETED: &str = "physical::enumerate::tests::stars_are_taken_only_within_the_budget";

/// Mechanisms of the implied edges whose removal must fail their tests.
pub const IMPLIED_EDGES: [Mutation; 5] = [
    Mutation {
        name: "implied-edges-of-any-column",
        package: Package::Core,
        breaks: "a column that is no key of its relation gives it a star, and a key none",
        file: MODEL_FILE,
        original: "class.iter().filter(|&&c| keyed(c))",
        replacement: "class.iter().filter(|&&c| !keyed(c))",
        integration: &["implied_edges::no_edge_is_implied_without_an_immediate_key"],
        unit: &[JOINED],
    },
    Mutation {
        name: "implied-edges-join-nothing",
        package: Package::Core,
        breaks: "an implied edge joins no two sets, so the keyed relation meets its class's others only through the stated conjuncts",
        file: GRAPH_FILE,
        original: "self.edges.iter().any(|e| e.joins(a, b)) || self.implies(a, b)",
        replacement: "self.edges.iter().any(|e| e.joins(a, b))",
        integration: &["implied_edges::a_declared_key_of_a_class_joins_its_other_relations"],
        unit: &[JOINED],
    },
    Mutation {
        name: "implied-edges-past-the-budget",
        package: Package::Core,
        breaks: "a star that passes the budget is kept, so a chain of keyed relations is planned as a clique",
        file: IMPLIED_FILE,
        original: "(self.simple, self.implied) = before;",
        replacement: "drop(before);",
        integration: &[],
        unit: &[BUDGETED],
    },
    Mutation {
        name: "implied-edges-most-rows-first",
        package: Package::Core,
        breaks: "the stars are offered from the relation of most rows, so the budget goes to the join that reduces least",
        file: MODEL_FILE,
        original: "self.rows(a.0).total_cmp(&self.rows(b.0))",
        replacement: "self.rows(b.0).total_cmp(&self.rows(a.0))",
        integration: &[],
        unit: &[BUDGETED],
    },
    Mutation {
        name: "implied-edges-deferrable-key",
        package: Package::Core,
        breaks: "the unique index of a deferrable constraint is a key to the estimates, so its column gives a star",
        file: "src/physical/cardinality/keys.rs",
        original: "indexed.filter(|key| !late.contains(key)).collect()",
        replacement: "indexed.collect()",
        integration: &["implied_edges::no_edge_is_implied_without_an_immediate_key"],
        unit: &[],
    },
];
