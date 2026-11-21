//! The mutations of streamed groupings and row goals (RULES.md R-TEST-9,
//! D550 to D552): a grouping by a collation kept hashed, a batch's runs
//! cut by their keys, set functions that cannot raise letting FETCH FIRST
//! stop, and a row goal passed through a filter.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the streamed groupings' and row
/// goals' requirements.
pub const ORDERS: [Mutation; 4] = [
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
];
