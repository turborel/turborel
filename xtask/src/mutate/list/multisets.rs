//! The mutations of multisets (RULES.md R-TEST-9, D80): equality false
//! of a pairing whose nulls exactly cover what the classes leave;
//! SUBMULTISET of more elements; a row holding a null paired only if equal; the matching never
//! augmented; distinctness of the counts not compared; EXCEPT DISTINCT
//! keeping what the other has; INTERSECTION's limit one early; MEMBER of
//! an empty multiset unknown; the grouped elements not refused; a
//! constraint's UNION ALL of strings refused; INTERSECT bound as loosely
//! as UNION; and ORDER BY in a constructor by query taken.

use super::{Mutation, Package};

/// The mutations of multisets, in the order they run.
pub const MULTISETS: [Mutation; 12] = [
    Mutation {
        name: "multiset-equal-uncovered",
        package: Package::Core,
        breaks: "two multisets are false, not unknown, though each element left by the classes pairs with a null exactly [§8.2 GR1b iii]",
        file: "src/value/multisets.rs",
        original: "covers the right too.\n    if matched.saturating_add(wy) >= lx {",
        replacement: "covers the right too.\n    if matched.saturating_add(wy) > lx {",
        integration: &["multisets::multisets_compared_8_2"],
        unit: &["value::multisets::tests::equality_8_2"],
    },
    Mutation {
        name: "multiset-submultiset-more",
        package: Package::Core,
        breaks: "a multiset of more elements is taken to be a submultiset [§8.17 GR3c i]",
        file: "src/value/multisets.rs",
        original: "    if xs.len() > ys.len() {",
        replacement: "    if xs.len() > ys.len().saturating_add(1) {",
        integration: &[],
        unit: &["value::multisets::tests::submultiset_8_17"],
    },
    Mutation {
        name: "multiset-row-pairs-unknown",
        package: Package::Core,
        breaks: "a row holding a null pairs with a row only if they are equal, so a pairing of unknown pairs is false [§8.2 GR7]",
        file: "src/value/multisets.rs",
        original: "            if rows_hold(std::slice::from_ref(*x), CompOp::Eq, std::slice::from_ref(*y)) != Some(false) {",
        replacement: "            if rows_hold(std::slice::from_ref(*x), CompOp::Eq, std::slice::from_ref(*y)) == Some(true) {",
        integration: &[],
        unit: &["value::multisets::tests::rows_holding_nulls_8_2"],
    },
    Mutation {
        name: "multiset-matching-greedy",
        package: Package::Core,
        breaks: "the matching never takes an element from the one it was paired with, so it misses a pairing",
        file: "src/value/multisets.rs",
        original: "            stack.push((w, 0));",
        replacement: "            stack.push((w, usize::MAX));",
        integration: &[],
        unit: &["value::multisets::tests::matching_augments"],
    },
    Mutation {
        name: "multiset-distinct-counts",
        package: Package::Core,
        breaks: "multisets of the same values in other counts are not distinct [§8.15 GR1c vi]",
        file: "src/value/multisets.rs",
        original: "    Ok(all.iter().any(|c| c.left != c.right))",
        replacement: "    Ok(all.iter().any(|c| (c.left == 0) != (c.right == 0)))",
        integration: &["multisets::multisets_compared_8_2"],
        unit: &["value::multisets::tests::distinctness_8_15"],
    },
    Mutation {
        name: "multiset-except-distinct",
        package: Package::Core,
        breaks: "MULTISET EXCEPT DISTINCT keeps a value the second operand has fewer of [§6.50 SR3d]",
        file: "src/value/multisets.rs",
        original: "            (BagOp::Except, true) => usize::from(c.right == 0),",
        replacement: "            (BagOp::Except, true) => usize::from(c.left > c.right),",
        integration: &["multisets::multiset_operators_6_50"],
        unit: &["value::multisets::tests::operations_6_50"],
    },
    Mutation {
        name: "multiset-gathered-limit",
        package: Package::Core,
        breaks: "COLLECT and FUSION overflow at MAX_MULTISET_CARDINALITY elements, not past it [§10.9 GR3b]",
        file: "src/functions/multisets.rs",
        original: "        if total > crate::value::types::multiset::most() {",
        replacement: "        if total >= crate::value::types::multiset::most() {",
        integration: &[],
        unit: &["functions::multisets::tests::overflow_past_the_limit"],
    },
    Mutation {
        name: "multiset-member-empty",
        package: Package::Core,
        breaks: "a null is unknown, not false, MEMBER OF an empty multiset [§8.16 GR3a]",
        file: "src/functions/multisets.rs",
        original: "        if mv.is_some_and(<[Value]>::is_empty) {",
        replacement: "        if mv.is_some_and(<[Value]>::is_empty) && !x.is_null() {",
        integration: &["multisets::member_predicate_8_16"],
        unit: &[],
    },
    Mutation {
        name: "multiset-grouped-taken",
        package: Package::Core,
        breaks: "a multiset of arrays is grouped by SET, the operators DISTINCT, EXCEPT and INTERSECT, = and IS A SET [§9.13 SR3]",
        file: "src/binder/multisets.rs",
        original: "    if let Some(these) = unordered(element) {",
        replacement: "    if let Some(these) = unordered(element).filter(|_| false) {",
        integration: &["multisets::grouping_refused_9_13"],
        unit: &[],
    },
    Mutation {
        name: "multiset-certain-all",
        package: Package::Core,
        breaks: "a constraint's MULTISET UNION ALL of character strings, which groups nothing, is refused as non-deterministic [§9.16 SR1n]",
        file: "src/binder/multisets.rs",
        original: "        self.certain(grouping && uncertain(&element),",
        replacement: "        self.certain(uncertain(&element),",
        integration: &["multiset_columns::constraints_are_deterministic_9_16"],
        unit: &[],
    },
    Mutation {
        name: "multiset-intersect-loose",
        package: Package::Core,
        breaks: "MULTISET INTERSECT binds as loosely as UNION and EXCEPT, read left to right with them [§6.50]",
        file: "src/parser/multisets.rs",
        original: "            let right = self.multiset_intersections(first)?;",
        replacement: "            let right = first;",
        integration: &["multisets::multiset_operators_6_50"],
        unit: &[],
    },
    Mutation {
        name: "multiset-query-ordered",
        package: Package::Core,
        breaks: "the query of a multiset value constructor by query may have ORDER BY [§6.52 SR1b, SR4b]",
        file: "src/binder/subquery.rs",
        original: "            && !o.order_by.is_empty()",
        replacement: "            && o.order_by.is_empty() && false",
        integration: &["multisets::constructed_by_query_6_52"],
        unit: &[],
    },
];
