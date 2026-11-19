//! The mutations of GRAPH_TABLE (RULES.md R-TEST-9, D141, D142): of its
//! binding, a variable's candidate tables and an anonymous element's
//! variable; of its run, a LEFT join's padding, the postfilter, DISTINCT
//! of a group's set function, ONE ROW PER STEP's steps, an OPTION's limit
//! and the warning of a truncated search; of its graph, the recheck of an
//! adjacency index's entry; and of the adjacency indexes an ENFORCED graph
//! is given. None makes a search run longer than its limits allow.

use super::{Mutation, Package};

pub(super) mod indexes;
pub(super) mod joins;
pub(super) mod statistics;

/// The mutations of GRAPH_TABLE, in the order they run.
pub const GRAPH_TABLE: [Mutation; 10] = [
    Mutation {
        name: "graph-table-candidates-of-every-label",
        package: Package::Core,
        breaks: "a variable written twice binds only the tables every occurrence's label holds of, so a union of two labels loses a vertex",
        file: "src/binder/graph/variables.rs",
        original: "    let any = |t: &TableInfo| d.labels.is_empty() || d.labels.iter().any(|l| l.as_ref().is_none_or(|l| label_holds(l, &t.labels)));",
        replacement: "    let any = |t: &TableInfo| d.labels.is_empty() || d.labels.iter().all(|l| l.as_ref().is_none_or(|l| label_holds(l, &t.labels)));",
        integration: &["graph_table::conformance::t5_t6_union_and_alternation_23_2"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-anonymous-bound",
        package: Package::Core,
        breaks: "an anonymous element is bound to a variable of the path, so the branches of a union that match one path give two bindings",
        file: "src/binder/graph/pattern.rs",
        original: "        let id = if found.is_some() || var.name.starts_with('$') {",
        replacement: "        let id = if found.is_some() {",
        integration: &["graph_table::conformance::t9_loops_23_2"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-left-unpadded",
        package: Package::Core,
        breaks: "a LEFT join's row of no match is dropped, not null extended",
        file: "src/exec/graph_table.rs",
        original: "                && self.u.outer.is_some()",
        replacement: "                && self.u.outer.is_some() && false",
        integration: &["graph_table::reads::lateral_reads_8"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-postfilter-ignored",
        package: Package::Core,
        breaks: "the WHERE after a pattern is not tested, so every match gives its rows",
        file: "src/exec/graph_table.rs",
        original: "            if let Some(c) = &self.graph.postfilter",
        replacement: "            if let Some(c) = self.graph.postfilter.as_ref().filter(|_| false)",
        integration: &["graph_table::conformance::t8_postfilter_23_2"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-distinct-ignored",
        package: Package::Core,
        breaks: "DISTINCT of a group variable's set function counts a repeated value again",
        file: "src/exec/graph_table/values.rs",
        original: "            if call.distinct {",
        replacement: "            if call.distinct && false {",
        integration: &["graph_table::functions::group_aggregates_4_4"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-step-shifted",
        package: Package::Core,
        breaks: "ONE ROW PER STEP binds an edge's own first vertex as the vertex after it",
        file: "src/exec/graph_table/values.rs",
        original: "Some((*vertices.get(i)?, Some(e), vertices.get(i.saturating_add(1)).copied()))",
        replacement: "Some((*vertices.get(i)?, Some(e), vertices.get(i).copied()))",
        integration: &["graph_table::reads::rows_per_vertex_and_step_8"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-option-length-ignored",
        package: Package::Core,
        breaks: "OPTIONS (MAX_PATH_LENGTH n) is not read, the setting alone bounding the search",
        file: "src/exec/graph_table/limits.rs",
        original: "        max_path_length: limit(options.max_path_length, s.count(GRAPH_MAX_PATH_LENGTH)),",
        replacement: "        max_path_length: limit(None, s.count(GRAPH_MAX_PATH_LENGTH)),",
        integration: &["graph_table::limits::options_bound_the_search_20"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-truncation-silent",
        package: Package::Core,
        breaks: "a search ended ON LIMIT TRUNCATE gives no warning 01W61",
        file: "src/graph/limits.rs",
        original: "        if let (true, Some(stop), Some(state)) = (self.truncated(), &self.stop, SqlState::parse(TRUNCATED_STATE)) {",
        replacement: "        if let (true, Some(stop), Some(state)) = (self.truncated() && false, &self.stop, SqlState::parse(TRUNCATED_STATE)) {",
        integration: &["graph_table::limits::options_bound_the_search_20"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-adjacency-unchecked",
        package: Package::Core,
        breaks: "an adjacency index's entry is followed without its row read again, so an edge whose source changed is still followed from the old one",
        file: "src/exec/graph_table/store/expand.rs",
        original: "                if key_text(&values_of(edge_row, &side.at.columns)) != key_text(&key.0) {",
        replacement: "                if key_text(&values_of(edge_row, &side.at.columns)) != key_text(&key.0) && false {",
        integration: &["graph_table::adjacency::indexed_matches_follow_changes_13_2"],
        unit: &[],
    },
    Mutation {
        name: "graph-table-enforced-unindexed",
        package: Package::Core,
        breaks: "an ENFORCED graph's edge tables are given no adjacency index as it commits",
        file: "src/db/graphs/adjacency.rs",
        original: "            if kept || !self.owns_schema(st, &element.table.auth) {",
        replacement: "            if kept || !self.owns_schema(st, &element.table.auth) || true {",
        integration: &["graph_table::adjacency::enforced_graphs_are_indexed_13_2"],
        unit: &[],
    },
];
