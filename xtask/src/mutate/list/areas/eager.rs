//! The mutations of eager aggregation (RULES.md R-TEST-9, D660 to D662):
//! a scaled sum not multiplied, sums of approximate numbers computed
//! above the join, an argument that may be null pushed, a part finished as
//! a result, grouping columns whose equal values may differ, a chain
//! made again with its conjuncts above its last relation, and a plan
//! grouped first grouped first again.

use super::super::{Mutation, Package};

/// The test comparing every grouping grouped first with it as written.
const GROUPED: &str = "eager_aggregation::groupings_grouped_first_are_the_groupings_10_9";

/// Mechanisms whose removal must fail eager aggregation's requirements.
pub const EAGER: [Mutation; 7] = [
    Mutation {
        name: "eager-scaled-unmultiplied",
        package: Package::Core,
        breaks: "a value of n rows is summed n times (Yan and Larson's class C)",
        file: "src/functions/partial/scaled.rs",
        original: "            ColumnarKind::Sum => put_value(&mut state, &times(value, rows)?),",
        replacement: "            ColumnarKind::Sum => put_value(&mut state, value),",
        integration: &[GROUPED],
        unit: &["functions::partial::scaled::tests::a_value_of_n_rows_is_n_values"],
    },
    Mutation {
        name: "eager-floats-allowed",
        package: Package::Core,
        breaks: "a sum of approximate numbers leaves the grouping as written",
        file: "src/physical/eager/classify.rs",
        original: "    if summed && !exact(&a.result) {\n        return None;\n    }",
        replacement: "",
        integration: &["eager_aggregation::groupings_eager_aggregation_leaves_alone"],
        unit: &["physical::eager::classify::tests::shapes_refused"],
    },
    Mutation {
        name: "eager-nulls-pushed",
        package: Package::Core,
        breaks: "an argument that may be null is computed above the join",
        file: "src/physical/eager/classify.rs",
        original: "!can_raise_typed(e, self.types) && self.never_null(e, 0));",
        replacement: "!can_raise_typed(e, self.types));",
        integration: &["eager_aggregation::nulls_eliminated_warn_as_the_join_does_6_5"],
        unit: &["physical::eager::classify::tests::functions_are_pushed_plain_or_scaled"],
    },
    Mutation {
        name: "eager-part-finished-as-result",
        package: Package::Core,
        breaks: "a part kept in a column is finished as its part",
        file: "src/exec/table/states.rs",
        original: "            States::Parted(typed) => typed.finish_part(g),",
        replacement: "            States::Parted(typed) => typed.finish(g),",
        integration: &[GROUPED],
        unit: &[],
    },
    Mutation {
        name: "eager-keys-unlike",
        package: Package::Core,
        breaks: "grouping columns whose equal values may differ leave the grouping as written",
        file: "src/physical/eager/classify.rs",
        original: "        let alike = keys.iter().all(|&k| (self.types)(k).as_ref().is_some_and(alike));",
        replacement: "        let alike = true;",
        integration: &["eager_aggregation::groupings_eager_aggregation_leaves_alone"],
        unit: &["physical::eager::classify::tests::shapes_refused"],
    },
    Mutation {
        name: "eager-conjuncts-on-top",
        package: Package::Core,
        breaks: "a chain made again joins each relation by its own conjuncts",
        file: "src/physical/eager/chain.rs",
        original: "    let (here, later): (Vec<_>, Vec<_>) = std::mem::take(pending).into_iter().partition(|(last, _)| *last < width);",
        replacement: "    let (here, later): (Vec<_>, Vec<_>) = std::mem::take(pending).into_iter().partition(|(last, _)| *last < width && width == usize::MAX);",
        integration: &[],
        unit: &["physical::eager::tests::chains_made_again_place_each_conjunct_lowest"],
    },
    Mutation {
        name: "eager-regrouped",
        package: Package::Core,
        breaks: "a plan grouped first is never grouped first again",
        file: "src/physical/eager.rs",
        original: "PhysicalPlanner { est: Some(&estimates), regrouped: true, ..*self }",
        replacement: "PhysicalPlanner { est: Some(&estimates), regrouped: false, ..*self }",
        integration: &["plan_space::a_grouping_is_grouped_first_once_under_any_cardinalities"],
        unit: &[],
    },
];
