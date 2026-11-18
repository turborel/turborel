//! The mutations of the graph indexes (RULES.md R-TEST-9, D115, D116): of
//! the reachability labels, an edge added and an edge taken back; of the
//! pruning, the starts that are ends and the way edges are followed; of
//! the label bitmaps, a negated label; of the label numbering, its
//! renumbering; and of the statements, an index's name and EXPLAIN's
//! line. None makes a search run longer than its limits allow.

use super::super::{Mutation, Package};

/// The mutations of the graph indexes, in the order they run.
pub const GRAPH_INDEXES: [Mutation; 8] = [
    Mutation {
        name: "graph-indexes-spread-unpruned-inverted",
        package: Package::Core,
        breaks: "an edge added marks only the vertices already answered, so a pair it joins is not reached",
        file: "src/index/reachability/landmarks.rs",
        original: "                if !before {",
        replacement: "                if before {",
        integration: &[],
        unit: &["index::reachability::tests::labels_kept_as_edges_are_added_stay_exact"],
    },
    Mutation {
        name: "graph-indexes-hub-unmarked",
        package: Package::Core,
        breaks: "an edge's source is not a landmark of its own, so it does not reach what the edge leads to",
        file: "src/index/reachability/landmarks.rs",
        original: "        self.mark(a, Way::Backward, rank);",
        replacement: "        self.mark(a, Way::Backward, rank.saturating_add(1));",
        integration: &[],
        unit: &["index::reachability::tests::a_chain_joined_in_the_middle_reaches_end_to_end"],
    },
    Mutation {
        name: "graph-indexes-end-start-dropped",
        package: Package::Core,
        breaks: "a start that is itself an end is dropped unless it reaches another, so a match of no edge is lost",
        file: "src/index/reachability/landmarks.rs",
        original: "                named.contains(key.as_slice()) || self",
        replacement: "                self",
        integration: &["graph_table::graph_indexes::pruned_starts_are_not_searched"],
        unit: &[],
    },
    Mutation {
        name: "graph-indexes-reverse-ignored",
        package: Package::Core,
        breaks: "a search of edges followed backward is pruned by what reaches forward, so its starts are lost",
        file: "src/index/reachability/landmarks.rs",
        original: "flat_map(|&e| self.label(e, reverse).to_vec())",
        replacement: "flat_map(|&e| self.label(e, false).to_vec())",
        integration: &["graph_table::graph_indexes::indexed_searches_give_the_rows_unindexed_ones_do"],
        unit: &[],
    },
    Mutation {
        name: "graph-indexes-removal-kept",
        package: Package::Core,
        breaks: "an edge taken back leaves its labels, so the index still says what it joined is reached",
        file: "src/index/reachability/state.rs",
        original: "        if matches!(self.state, State::Made(..)) {",
        replacement: "        if matches!(self.state, State::Stale) {",
        integration: &[],
        unit: &["index::reachability::tests::a_removal_drops_the_labels_until_maintenance"],
    },
    Mutation {
        name: "graph-indexes-negation-ignored",
        package: Package::Core,
        breaks: "a negated label is read as the label, so `!Account` finds the accounts",
        file: "src/exec/graph_table/labels.rs",
        original: "            LabelExpr::Not(e) => !self.holds(e, n),",
        replacement: "            LabelExpr::Not(e) => self.holds(e, n),",
        integration: &["graph_table::graph_indexes::indexed_searches_give_the_rows_unindexed_ones_do"],
        unit: &[],
    },
    Mutation {
        name: "graph-indexes-renumbered-never",
        package: Package::Core,
        breaks: "maintenance never numbers the rows again, however many numbers are of rows gone",
        file: "src/index/labels/numbered.rs",
        original: "            (Some(made), _) => made.dead > LEAST_DEAD",
        replacement: "            (Some(made), _) => made.dead > usize::MAX - 1 + 0 * LEAST_DEAD",
        integration: &[],
        unit: &["index::labels::tests::maintenance_numbers_the_rows_again_once_many_are_gone"],
    },
    Mutation {
        name: "graph-indexes-explain-unnamed",
        package: Package::Core,
        breaks: "EXPLAIN does not say a path's starts are pruned",
        file: "src/logical/graph/indexes.rs",
        original: "starts pruned by reachability index",
        replacement: "starts searched",
        integration: &["graph_table::graph_indexes::explain_names_the_indexes_read"],
        unit: &[],
    },
];
