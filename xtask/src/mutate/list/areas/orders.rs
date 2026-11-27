//! The mutations of streamed groupings and row goals (RULES.md R-TEST-9,
//! D550 to D552): a grouping by a collation kept hashed, a batch's runs
//! cut by their keys, set functions that cannot raise letting FETCH FIRST
//! stop, a row goal passed through a filter, and none passed to the input
//! of a sort that reads it whole (D1218), and a sort of a grouping's input
//! weighed by the rows of the plan built (D1208), by their groups, with
//! each form priced as it would run, and by least regret (D1278).

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the streamed groupings' and row
/// goals' requirements.
pub const ORDERS: [Mutation; 10] = [
    Mutation {
        name: "streamed-collated",
        package: Package::Core,
        breaks: "a grouping by a collation is hashed, not streamed",
        file: "src/physical/streamed.rs",
        original: "    collations.iter().all(Option::is_none) && seeds == 0 && !alone",
        replacement: "    seeds == 0 && !alone",
        integration: &[],
        unit: &["physical::streamed::tests::a_collation_seeds_or_a_distinct_function_keep_a_grouping_hashed"],
    },
    Mutation {
        name: "streamed-runs-uncut",
        package: Package::Core,
        breaks: "a batch's rows of a new key begin a new group",
        file: "src/exec/batch/groups/streamed.rs",
        original: "            if states.is_some() && how.keys.equal(0, key, &mut self.bytes) {",
        replacement: "            if states.is_some() {",
        integration: &[],
        unit: &["exec::batch::groups::streamed::tests::runs_of_one_key_are_one_group"],
    },
    Mutation {
        name: "streamed-count-raises",
        package: Package::Core,
        breaks: "COUNT, MIN and MAX cannot raise, so FETCH FIRST stops the grouping of them",
        file: "src/physical/raising.rs",
        original: "        Some(ColumnarKind::Count) | Some(ColumnarKind::Min) | Some(ColumnarKind::Max) => false,",
        replacement: "        Some(ColumnarKind::Count) | Some(ColumnarKind::Min) | Some(ColumnarKind::Max) => true,",
        integration: &["streamed_grouping::fetch_first_stops_a_streamed_grouping"],
        unit: &[],
    },
    Mutation {
        name: "row-goal-stopped",
        package: Package::Core,
        breaks: "a row goal passes through the nodes that give their rows as they read them",
        file: "src/physical.rs",
        original: "        if self.first.is_some() && !goals::passes(logical) {",
        replacement: "        if self.first.is_some() {",
        integration: &["row_goals::a_goal_passes_through_a_filter", "row_goals::a_goal_passes_to_a_joins_left_operand"],
        unit: &[],
    },
    Mutation {
        name: "row-goal-under-sort",
        package: Package::Core,
        breaks: "the input of a sort that reads it whole is planned for the few rows FETCH FIRST asks of the sort",
        file: "src/physical/ordered.rs",
        original: "if b.ordered || self.first.is_none() { b } else { self.whole().build(input, keys) }",
        replacement: "b",
        integration: &["streamed_grouping::a_sort_that_reads_every_group_passes_no_row_goal"],
        unit: &[],
    },
    Mutation {
        name: "streamed-sort-weighed-by-logical-rows",
        package: Package::Core,
        breaks: "sorting a grouping's input first is weighed by the logical input's rows, not those of the plan built",
        file: "src/physical/streamed.rs",
        original: "let n = input.est().unwrap_or_else(|| self.card(g.input).rows);",
        replacement: "let n = self.card(g.input).rows;",
        integration: &[],
        unit: &["physical::streamed::tests::a_sort_first_is_weighed_by_the_rows_of_the_plan_built"],
    },
    Mutation {
        name: "streamed-hashed-priced-alone",
        package: Package::Core,
        breaks: "a hashed grouping its part's threads share is priced as run on one, against a sort of its rows",
        file: "src/physical/streamed.rs",
        original: "let hashed = |made: f64| hash.cost(self.hashed_cost(n, Grouping { groups: made, ..g })) + w.sort(made);",
        replacement: "let hashed = |made: f64| self.hashed_cost(n, Grouping { groups: made, ..g }) + w.sort(made);",
        integration: &[],
        unit: &["physical::streamed::tests::a_sort_first_is_weighed_as_each_form_would_run"],
    },
    Mutation {
        name: "streamed-groups-of-logical-rows",
        package: Package::Core,
        breaks: "sorting a grouping's input first is weighed by the groups of the logical input's rows, not of those built",
        file: "src/physical/streamed.rs",
        original: "let groups = g.groups.min(self.estimating(|est| est.groups_of(g.input, g.keys, n)));",
        replacement: "let groups = g.groups;",
        integration: &[],
        unit: &["physical::streamed::tests::a_sort_first_is_weighed_as_each_form_would_run"],
    },
    Mutation {
        name: "streamed-weighed-at-the-estimate-alone",
        package: Package::Core,
        breaks: "sorting a grouping's input first is taken where it is the cheaper at the groups estimated, whatever it costs of fewer",
        file: "src/physical/streamed.rs",
        original: "let few = (groups / w.estimate_margin.max(1.0_f64)).max(groups.min(1.0_f64));",
        replacement: "let few = groups;",
        integration: &[],
        unit: &["physical::streamed::tests::a_sort_first_is_weighed_as_each_form_would_run"],
    },
    Mutation {
        name: "streamed-sort-priced-as-the-grouping-runs",
        package: Package::Core,
        breaks: "a sort left on the statement's thread is priced as shared by the threads of the grouping's part",
        file: "src/physical/streamed.rs",
        original: "let sort = Run::shared(sides::sort_degree(db, seen, input), gain);",
        replacement: "let sort = Run::shared(sides::part_degree(db, seen, &make(input.clone(), false)), gain);",
        integration: &["streamed_grouping::a_grouping_whose_order_is_asked_is_weighed_as_it_would_run"],
        unit: &[],
    },
];
