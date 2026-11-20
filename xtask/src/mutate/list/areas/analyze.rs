//! The mutations of EXPLAIN ANALYZE and of cardinalities given the planner
//! (RULES.md R-TEST-9, D376 to D378): an operator's rows lost as its
//! iterator is dropped, a step of a pipeline run in parallel or of one by
//! batches that counts nothing, times shown while the clock stood still,
//! ANALYZE read after EXPLAIN before any token, and the rows given left
//! out by the estimator or by the join enumeration.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the requirements of the forms.
pub const ANALYZE: [Mutation; 7] = [
    Mutation {
        name: "analyze-rows-lost",
        package: Package::Core,
        breaks: "an operator's rows are lost as its iterator is dropped",
        file: "src/exec/counted.rs",
        original: "        self.count.add(self.rows, self.micros);",
        replacement: "        self.count.add(0, self.micros);",
        integration: &["explain_analyze::a_scan_shows_its_rows_beside_its_estimate"],
        unit: &[],
    },
    Mutation {
        name: "analyze-step-uncounted",
        package: Package::Core,
        breaks: "an operator of a pipeline run in parallel a row at a time counts no rows",
        file: "src/exec/parallel/pipeline.rs",
        original: "            count.stepped(rows);",
        replacement: "            count.stepped(rows.min(0));",
        integration: &["explain_analyze::a_part_run_in_parallel_sums_its_threads_counts"],
        unit: &[],
    },
    Mutation {
        name: "analyze-batch-step-uncounted",
        package: Package::Core,
        breaks: "an operator of a pipeline run in parallel by batches counts no rows",
        file: "src/exec/batch/shaped.rs",
        original: "count.stepped(u64::try_from(rows.as_ref().map_or(batch.rows, Sel::len)).unwrap_or(u64::MAX));",
        replacement: "count.stepped(u64::try_from(rows.as_ref().map_or(batch.rows, Sel::len)).unwrap_or(u64::MAX).min(0));",
        integration: &["explain_analyze::a_part_run_in_parallel_sums_its_threads_counts"],
        unit: &[],
    },
    Mutation {
        name: "analyze-time-always",
        package: Package::Core,
        breaks: "an operator's time is hidden though the clock moved",
        file: "src/physical/analyzed.rs",
        original: "        self.timed.store(moved, Ordering::Relaxed);",
        replacement: "        self.timed.store(moved && false, Ordering::Relaxed);",
        integration: &["explain_analyze::times_are_the_clocks"],
        unit: &["physical::shown::tests::a_counted_operator_shows_what_it_did"],
    },
    Mutation {
        name: "analyze-any-token",
        package: Package::Core,
        breaks: "ANALYZE after EXPLAIN is read as the option before a parenthesis that begins no query",
        file: "src/parser/data_statements.rs",
        original: "(analyze && begins_query(third) && (!parenthesis || begins_query(fourth)))",
        replacement: "(analyze && begins_query(third))",
        integration: &["explain_analyze::analyze_is_read_as_the_option_only_before_a_query"],
        unit: &["parser::data_statements::tests::analyze_is_read_right_after_explain_before_a_query"],
    },
    Mutation {
        name: "injected-estimate-ignored",
        package: Package::Core,
        breaks: "the estimator leaves out the rows given of a table's read",
        file: "src/physical/cardinality/nodes.rs",
        original: "        let card = self.injected(plan).unwrap_or_else(|| self.found(plan, depth));",
        replacement: "        let card = self.injected(plan).filter(|_| false).unwrap_or_else(|| self.found(plan, depth));",
        integration: &["explain_analyze::injected_cardinalities_change_the_plan_not_the_rows"],
        unit: &[],
    },
    Mutation {
        name: "injected-join-ignored",
        package: Package::Core,
        breaks: "the join enumeration leaves out the rows given of a join",
        file: "src/physical/enumerate/model.rs",
        original: "        if let Some(&(_, given)) = self.injected.iter().find(|(s, _)| *s == s1 | s2) {",
        replacement: "        if let Some(&(_, given)) = self.injected.iter().find(|(s, _)| *s == s1 | s2 && false) {",
        integration: &["explain_analyze::injected_cardinalities_change_the_plan_not_the_rows"],
        unit: &[],
    },
];
