//! The mutations of how a path search is chosen and how it expands a
//! frontier (RULES.md R-TEST-9, D118): MS-BFS chosen by cost, a start's
//! estimated reach, workers used when the work pays, a PATH_ALGO hint, a
//! level's hops asked at once, a frontier's lists read of the compacted
//! adjacency, and the store's hops of a frontier put back by vertex. None
//! makes a search run longer than its limits allow.

use super::super::{Mutation, Package};

/// The mutations of search choice and the vectorized Expand, in the order
/// they run.
pub const GRAPH_SEARCH_CHOICE: [Mutation; 7] = [
    Mutation {
        name: "graph-choice-multi-source-always",
        package: Package::Core,
        breaks: "MS-BFS is chosen for many starts whose reaches do not cover the graph, where BFS from each costs less",
        file: "src/graph/search/choice.rs",
        original: "    if batch.saturating_mul(e.reach) >= e.vertices { Algorithm::MultiSourceBfs } else { Algorithm::Bfs }",
        replacement: "    if batch.saturating_mul(e.reach) >= 0 { Algorithm::MultiSourceBfs } else { Algorithm::Bfs }",
        integration: &[],
        unit: &["graph::search::tests::choice::multi_source_is_chosen_by_cost"],
    },
    Mutation {
        name: "graph-choice-reach-unbounded",
        package: Package::Core,
        breaks: "a start's reach is estimated past MAX_PATH_LENGTH, as if its walks were not bounded",
        file: "src/graph/search/choice.rs",
        original: "    for _ in 0..longest.unwrap_or(MOST_LEVELS).min(MOST_LEVELS) {",
        replacement: "    for _ in 0..MOST_LEVELS {",
        integration: &[],
        unit: &["graph::search::tests::choice::reach_is_estimated_from_the_degree"],
    },
    Mutation {
        name: "graph-choice-workers-always",
        package: Package::Core,
        breaks: "a search runs on its workers however little its work, below GRAPH.PARALLEL_MIN_STATES",
        file: "src/graph/search/choice.rs",
        original: "        let small = work < u64::try_from(self.options.parallel_min_states).unwrap_or(u64::MAX);",
        replacement: "        let small = work < u64::try_from(self.options.parallel_min_states).unwrap_or(u64::MAX) && false;",
        integration: &["graph_table::search_choice::explain_shows_the_choice"],
        unit: &["graph::search::tests::choice::workers_are_used_when_they_pay"],
    },
    Mutation {
        name: "graph-choice-hint-ignored",
        package: Package::Core,
        breaks: "a PATH_ALGO hint is ignored: the algorithm is chosen by cost, and a hint that cannot answer the pattern is not refused",
        file: "src/graph/search/choice.rs",
        original: "            Some(hint) => hinted(natural, hint, point)?,",
        replacement: "            Some(_) => by_cost(natural, estimate),",
        integration: &["graph_table::search_choice::hints_that_cannot_answer_are_refused"],
        unit: &["graph::search::tests::choice::hints_choose_within_the_family"],
    },
    Mutation {
        name: "graph-expand-one-at-a-time",
        package: Package::Core,
        breaks: "a level's hops are never asked at once, so each vertex's are read alone",
        file: "src/graph/search/expand.rs",
        original: "    if at.checked_rem(EXPAND_BATCH) != Some(0) {",
        replacement: "    if at.checked_rem(EXPAND_BATCH) != Some(0) || true {",
        integration: &[],
        unit: &["graph::search::tests::choice::frontiers_are_expanded_at_once"],
    },
    Mutation {
        name: "graph-expand-compacted-misplaced",
        package: Package::Core,
        breaks: "a frontier's lists read of the compacted adjacency are put at the wrong vertex's place",
        file: "src/index/adjacency/csr.rs",
        original: "            let Some(list) = lists.get_mut(place) else { continue };",
        replacement: "            let Some(list) = lists.get_mut(place / 2) else { continue };",
        integration: &[],
        unit: &["index::adjacency::tests::batch::lists_read_at_once_are_each_alone"],
    },
    Mutation {
        name: "graph-expand-store-misplaced",
        package: Package::Core,
        breaks: "the hops of a frontier's vertices are all given to its first vertex",
        file: "src/exec/graph_table/store/expand.rs",
        original: "            let (Some(&(vertex, _)), Some(hops)) = (frontier.asked.get(place), found.get_mut(place)) else { continue };",
        replacement: "            let (Some(&(vertex, _)), Some(hops)) = (frontier.asked.get(place), found.get_mut(0)) else { continue };",
        integration: &["graph_table::search_choice::every_choice_gives_the_same_rows"],
        unit: &[],
    },
];
