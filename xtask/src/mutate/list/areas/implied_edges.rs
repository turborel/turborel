//! The mutations of the edges a class of equalities implies (RULES.md
//! R-TEST-9, D1204): a star taken from a column that is no key, the
//! implied edges joining nothing, a star past the budget kept, the stars
//! offered from the relation of most rows, and the unique index of a
//! deferrable constraint taken for a key (D1205); and of the keys an inner
//! hash join hashes (D1206): every key of a column hashed, the key of
//! fewest distinct values kept, a moved key judged nowhere by either
//! path, and an equality taken for implied though no join below judged
//! it.

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

/// The keys hashed, one a column.
const SINGLE_FILE: &str = "src/physical/join/single.rs";
/// The chain's joins made plans.
const REALIZE_FILE: &str = "src/physical/enumerate/realize.rs";
/// The integration test of the key hashed and the one judged after.
const ONE_KEY: &str = "single_keys::an_inner_hash_join_hashes_one_key_of_a_column";
/// The integration test of an equality nothing below implies.
const STAYS: &str = "single_keys::an_equality_no_join_below_judged_stays_in_the_filter";

/// Mechanisms of the implied edges whose removal must fail their tests.
pub const IMPLIED_EDGES: [Mutation; 10] = [
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
    Mutation {
        name: "single-keys-every-key-hashed",
        package: Package::Core,
        breaks: "an inner hash join hashes every equality, two of one column too",
        file: SINGLE_FILE,
        original: "t.left != key.left && t.right != key.right",
        replacement: "t.left != key.left || t.right != key.right",
        integration: &[ONE_KEY],
        unit: &["physical::join::single::tests::a_column_repeated_keeps_one_key"],
    },
    Mutation {
        name: "single-keys-least-distinct-kept",
        package: Package::Core,
        breaks: "of the equalities of one column the one of fewest distinct values is hashed, which leaves the most pairs to judge",
        file: SINGLE_FILE,
        original: "b.1.total_cmp(&a.1)",
        replacement: "a.1.total_cmp(&b.1)",
        integration: &[ONE_KEY],
        unit: &["physical::join::single::tests::the_key_of_most_distinct_values_is_kept"],
    },
    Mutation {
        name: "single-keys-moved-key-unjudged",
        package: Package::Core,
        breaks: "an equality the enumerated join does not hash is judged nowhere, so pairs it refuses are joined",
        file: REALIZE_FILE,
        original: ".filter(|&m| !cx.implied((&l, &r), &keys, m))",
        replacement: ".filter(|&m| cx.implied((&l, &r), &keys, m))",
        integration: &[STAYS],
        unit: &[],
    },
    Mutation {
        name: "single-keys-greedy-key-unjudged",
        package: Package::Core,
        breaks: "an equality the join planned without the enumeration does not hash is judged nowhere",
        file: "src/physical/join.rs",
        original: "rest.extend(moved.into_iter().map(|k| judged(k, width)));",
        replacement: "rest.extend(moved.into_iter().filter(|k| k.nulls).map(|k| judged(k, width)));",
        integration: &[ONE_KEY],
        unit: &[],
    },
    Mutation {
        name: "single-keys-implied-unjudged-below",
        package: Package::Core,
        breaks: "an equality is taken for implied though no join of the operand judged its columns equal, and is judged nowhere",
        file: REALIZE_FILE,
        original: "(Some(x), Some(y)) => chained(&side.equated, x, y),",
        replacement: "(Some(x), Some(y)) => x != y,",
        integration: &[STAYS],
        unit: &[],
    },
];
