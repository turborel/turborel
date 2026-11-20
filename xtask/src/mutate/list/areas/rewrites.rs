//! The mutations of the optimizer's rewrites (RULES.md R-TEST-9, D373 to
//! D375): the rules' places among the rules, the guard of a hoisted part
//! that can raise, the typed bound of exact arithmetic, the filters that
//! may go into a set operation or below a window, the FETCH FIRST that may
//! go into UNION ALL, and the keys a join is removed on.

use super::super::{Mutation, Package};

/// The test of hoisted parts raising as the product would.
const RAISING: &str = "rewrites::hoisting::hoisted_parts_raise_as_the_product_would";

/// The test of joins the rule must keep.
const KEPT: &str = "rewrites::elimination::joins_that_could_change_rows_are_kept";

/// Mechanisms whose removal must fail the rewrites' requirements.
pub const REWRITES: [Mutation; 12] = [
    Mutation {
        name: "rewrites-hoisting-unregistered",
        package: Package::Core,
        breaks: "the hoisting rule is among the built-in rules",
        file: "src/optimizer.rs",
        original: "        o.add(Box::new(HoistRaisingExpressions));",
        replacement: "",
        integration: &["rewrites::hoisting::hoisted_parts_let_chains_be_enumerated"],
        unit: &[],
    },
    Mutation {
        name: "rewrites-hoisting-unguarded",
        package: Package::Core,
        breaks: "a hoisted part that can raise is computed over a guard of the other operands",
        file: "src/optimizer/hoisting.rs",
        original: "            *guarded.get_mut(at)? |= can_raise_typed(&own, &|c| plan_column_type(plan, c, catalog));",
        replacement: "            *guarded.get_mut(at)? |= can_raise_typed(&own, &|c| plan_column_type(plan, c, catalog)) && at > count;",
        integration: &[RAISING],
        unit: &["optimizer::hoisting::tests::parts_are_columns_of_their_operands"],
    },
    Mutation {
        name: "rewrites-hoisting-outer-guarded",
        package: Package::Core,
        breaks: "no guard is made over an operand of an outer reference",
        file: "src/optimizer/hoisting.rs",
        original: "        if guarded.contains(&true) && operands.plans.iter().any(|o| o.width() == 0 || plan_has_outer(o)) {",
        replacement: "        if guarded.contains(&true) && operands.plans.iter().any(|o| o.width() == 0) {",
        integration: &[],
        unit: &["optimizer::hoisting::tests::what_cannot_be_taken_is_left"],
    },
    Mutation {
        name: "rewrites-typed-bound-widened",
        package: Package::Core,
        breaks: "exact arithmetic past 18 digits can raise",
        file: "src/logical/raises.rs",
        original: "    (precision <= MAX_EXACT_PRECISION).then_some((precision, scale))",
        replacement: "    (precision <= MAX_EXACT_PRECISION + 1).then_some((precision, scale))",
        integration: &[],
        unit: &["logical::raises::tests::sums_raise_past_eighteen_digits"],
    },
    Mutation {
        name: "rewrites-set-operation-raising",
        package: Package::Core,
        breaks: "a filter that can raise stays over INTERSECT and EXCEPT",
        file: "src/optimizer/setops.rs",
        original: "            SetOp::Intersect | SetOp::Except => alike(cond) && !cond.can_raise(),",
        replacement: "            SetOp::Intersect | SetOp::Except => alike(cond),",
        integration: &["rewrites::pushdown::filters_go_into_set_operations"],
        unit: &[],
    },
    Mutation {
        name: "rewrites-set-operation-unlike",
        package: Package::Core,
        breaks: "a filter that tells rows not distinct apart stays over UNION",
        file: "src/optimizer/setops.rs",
        original: "            SetOp::Union => *all || alike(cond),",
        replacement: "            SetOp::Union => true,",
        integration: &["rewrites::pushdown::filters_go_into_set_operations"],
        unit: &[],
    },
    Mutation {
        name: "rewrites-window-raising-kept",
        package: Package::Core,
        breaks: "no conjunct goes below a window beside one left above that can raise",
        file: "src/optimizer/windows.rs",
        original: "        if down.is_empty() || keep.iter().any(Cond::can_raise) {",
        replacement: "        if down.is_empty() {",
        integration: &["rewrites::pushdown::filters_go_below_window_partitions"],
        unit: &[],
    },
    Mutation {
        name: "rewrites-window-any-column",
        package: Package::Core,
        breaks: "only conjuncts over the partitioning columns go below a window",
        file: "src/optimizer/windows.rs",
        original: "    columns.iter().all(|col| partition.contains(col)) && alike(c)",
        replacement: "    !partition.is_empty() && alike(c)",
        integration: &["rewrites::pushdown::filters_go_below_window_partitions"],
        unit: &[],
    },
    Mutation {
        name: "rewrites-limit-ties",
        package: Package::Core,
        breaks: "FETCH FIRST WITH TIES stays over UNION ALL",
        file: "src/optimizer/limits.rs",
        original: "    if fetch.ties.is_some() || fetch.percent || matches!(fetch.search, Search::Approximate(_)) {",
        replacement: "    if fetch.percent || matches!(fetch.search, Search::Approximate(_)) {",
        integration: &["rewrites::pushdown::limits_go_into_union_all"],
        unit: &[],
    },
    Mutation {
        name: "rewrites-elimination-deferrable-key",
        package: Package::Core,
        breaks: "a join is not removed on a deferrable unique key",
        file: "src/optimizer/elimination.rs",
        original: "        !c.attributes.deferrable && key.iter().all(|k| columns.contains(k) && never_null(desc, *k) && uncollated(desc, *k))",
        replacement: "        key.iter().all(|k| columns.contains(k) && never_null(desc, *k) && uncollated(desc, *k))",
        integration: &[KEPT],
        unit: &["optimizer::elimination::tests::keys_must_be_unique_not_null_and_not_deferred"],
    },
    Mutation {
        name: "rewrites-elimination-nullable-key",
        package: Package::Core,
        breaks: "a join is not removed on a nullable unique key",
        file: "src/optimizer/elimination.rs",
        original: "        !c.attributes.deferrable && key.iter().all(|k| columns.contains(k) && never_null(desc, *k) && uncollated(desc, *k))",
        replacement: "        !c.attributes.deferrable && key.iter().all(|k| columns.contains(k) && uncollated(desc, *k))",
        integration: &[KEPT],
        unit: &["optimizer::elimination::tests::keys_must_be_unique_not_null_and_not_deferred"],
    },
    Mutation {
        name: "rewrites-elimination-deferrable-reference",
        package: Package::Core,
        breaks: "an inner join is not removed on a deferrable referential constraint",
        file: "src/optimizer/elimination/referential.rs",
        original: "    let immediate = desc.constraints.iter().any(|c| &c.name == name && !c.attributes.deferrable);",
        replacement: "    let immediate = desc.constraints.iter().any(|c| &c.name == name);",
        integration: &[KEPT],
        unit: &[],
    },
];
