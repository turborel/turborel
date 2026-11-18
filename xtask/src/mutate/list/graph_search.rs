//! The mutations of path search (RULES.md R-TEST-9): of the automaton, a
//! multiset alternation's tags and the termination rule R7.3; of the
//! runner, TRAIL's check of an edge and a singleton's join; of the
//! selectors, ALL SHORTEST's partition; of the searches, a negative cost,
//! where BiBFS's backward side begins, MS-BFS's choice of parent, and the order of
//! parallel results; of the limits, MAX_SEARCH_STATES and the starts after
//! a truncation; and of match modes, a group variable's repeat.

use super::{Mutation, Package};

pub(super) mod algorithms;

/// The mutations of path search, in the order they run.
pub const GRAPH_SEARCH: [Mutation; 12] = [
    Mutation {
        name: "graph-alternation-untagged",
        package: Package::Core,
        breaks: "a multiset alternation's branches are not tagged, so a binding both match is given once, as union gives it",
        file: "src/graph/nfa.rs",
        original: "                    entries.push(self.push(Instr::Tag { tag: *tag, next: entry })?);",
        replacement: "                    entries.push(self.push(Instr::Tag { tag: 0 * *tag, next: entry })?);",
        integration: &[],
        unit: &["graph::search::tests::bank::t5_t6_union_dedups_and_alternation_keeps"],
    },
    Mutation {
        name: "graph-walk-without-selector-allowed",
        package: Package::Core,
        breaks: "an unbounded quantifier within no restrictor or selector is accepted, so a search of WALK may not end",
        file: "src/graph/nfa/registers.rs",
        original: "            if open && selector == Selector::All && !self.restricted(scope, scopes) {",
        replacement: "            if open && selector == Selector::All && !self.restricted(scope, scopes) && false {",
        integration: &[],
        unit: &["graph::search::tests::bank::t4_walk_without_selector_is_grf_0031"],
    },
    Mutation {
        name: "graph-trail-repeats-edge",
        package: Package::Core,
        breaks: "TRAIL looks at the vertices of the path so far, not its edges, so it keeps only acyclic paths",
        file: "src/graph/run.rs",
        original: "                (!if whole { self.on(edge) } else { self.edges.get(mark.start..).unwrap_or_default().contains(&edge) }, false)",
        replacement: "                (!if whole { self.on(vertex) } else { since(mark.start).contains(&vertex) }, false)",
        integration: &[],
        unit: &[
            "graph::search::tests::bank::t1_t2_t3_restrictors_and_any_shortest",
            "graph::search::tests::oracle::every_restrictor_and_selector_agree_with_brute_force",
        ],
    },
    Mutation {
        name: "graph-singleton-unjoined",
        package: Package::Core,
        breaks: "a singleton variable's second binding is not compared with its first, so a repeated variable does not join",
        file: "src/graph/run.rs",
        original: "            if *first != element {",
        replacement: "            if *first != element && false {",
        integration: &[],
        unit: &["graph::search::tests::bank::t10_repeated_variable_joins"],
    },
    Mutation {
        name: "graph-all-shortest-keeps-longer",
        package: Package::Core,
        breaks: "ALL SHORTEST keeps a path of a partition longer than its first",
        file: "src/graph/search/selection.rs",
        original: "            Selector::AllShortest | Selector::AllCheapest => part.measures.first().is_none_or(|&m| m == measure),",
        replacement: "            Selector::AllShortest | Selector::AllCheapest => part.measures.first().is_none_or(|&m| m <= measure),",
        integration: &[],
        unit: &["graph::search::tests::oracle::every_restrictor_and_selector_agree_with_brute_force"],
    },
    Mutation {
        name: "graph-negative-cost-accepted",
        package: Package::Core,
        breaks: "a negative edge cost is taken, not GRF-0034, so Dijkstra's order is wrong",
        file: "src/graph/search/ranked.rs",
        original: "    if c.is_finite() && c >= 0.0 { Ok(c) } else { Err(cost_error(c)) }",
        replacement: "    if c.is_finite() || c >= 0.0 { Ok(c) } else { Err(cost_error(c)) }",
        integration: &[],
        unit: &["graph::search::tests::runs::negative_cost_is_grf_0034"],
    },
    Mutation {
        name: "graph-bibfs-backward-from-start",
        package: Package::Core,
        breaks: "BiBFS's backward side begins at the start, not the end, so it meets walks that end elsewhere",
        file: "src/graph/search/bidirectional.rs",
        original: "    behind.reach((end, accept), None, 0, meter)?;",
        replacement: "    behind.reach((start, accept), None, 0, meter)?;",
        integration: &[],
        unit: &["graph::search::tests::runs::bidirectional_equals_breadth_first"],
    },
    Mutation {
        name: "graph-ms-bfs-latest-parent",
        package: Package::Core,
        breaks: "MS-BFS keeps, of the parents a lane reaches a state from, the last, so a start's walk differs from its own search's",
        file: "src/graph/search/multi.rs",
        original: "        if key < entry.0 {",
        replacement: "        if key > entry.0 {",
        integration: &[],
        unit: &["graph::search::tests::runs::multi_source_equals_one_search_per_start"],
    },
    Mutation {
        name: "graph-parallel-results-unordered",
        package: Package::Core,
        breaks: "parallel morsels' results are put back in the order the workers finish, not the morsels'",
        file: "src/graph/search/parallel.rs",
        original: "    done.sort_unstable_by_key(|d| d.0);",
        replacement: "    done.sort_unstable_by_key(|d| std::cmp::Reverse(d.0));",
        integration: &[],
        unit: &["graph::search::tests::runs::parallel_searches_equal_sequential"],
    },
    Mutation {
        name: "graph-state-limit-ignored",
        package: Package::Core,
        breaks: "MAX_SEARCH_STATES is not looked at, so a search past it neither fails nor truncates",
        file: "src/graph/limits.rs",
        original: "        if let Some(limit) = self.limits.max_search_states.filter(|&limit| states > limit) {",
        replacement: "        if let Some(limit) = self.limits.max_search_states.filter(|&limit| states > limit.saturating_mul(1_000_000)) {",
        integration: &[],
        unit: &["graph::search::tests::runs::search_state_limit_errors_or_truncates"],
    },
    Mutation {
        name: "graph-truncated-search-goes-on",
        package: Package::Core,
        breaks: "a search truncated by a limit goes on to the next starts, giving more paths after the limit",
        file: "src/graph/search.rs",
        original: "        for &start in starts {\n            if meter.truncated() {",
        replacement: "        for &start in starts {\n            if meter.truncated() && false {",
        integration: &[],
        unit: &["graph::search::tests::runs::search_state_limit_errors_or_truncates"],
    },
    Mutation {
        name: "graph-group-variable-joins",
        package: Package::Core,
        breaks: "a group variable binding one edge twice counts as a join, so DIFFERENT EDGES keeps a repeated edge",
        file: "src/graph/matching.rs",
        original: "            let singles = vars.iter().filter(|v| !groups.contains(v));",
        replacement: "            let singles = vars.iter().filter(|v| groups.contains(v) || true);",
        integration: &[],
        unit: &["graph::search::tests::runs::group_variable_repeat_is_not_a_join"],
    },
];
