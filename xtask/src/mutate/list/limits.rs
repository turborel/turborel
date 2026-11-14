//! The mutations of OFFSET and FETCH FIRST (RULES.md R-TEST-9): rows not
//! passed over, one more kept, a null count or one below its least taken,
//! SR3j ix 3 and ix 4 dropped, a query of OFFSET or FETCH FIRST updatable,
//! an OFFSET over a work table found of the rows an iteration adds, a
//! recursive query of ORDER BY losing its plan, and OFFSET read as a
//! correlation name before its count.

use super::{Mutation, Package};

/// The file of OFFSET and FETCH FIRST run.
const RUN: &str = "src/exec/limit.rs";
/// The file of a recursive query's strata.
const STRATA: &str = "src/binder/strata.rs";
/// The file of a recursive query's plans.
const RECURSIVE: &str = "src/binder/recursive.rs";
/// The unit test of the rows kept.
const KEPT: &str = "exec::limit::tests::the_rows_offset_and_fetch_first_keep_7_17";
/// The unit test of the counts refused.
const REFUSED: &str = "exec::limit::tests::a_count_not_a_row_count_is_refused_7_17";
/// The test of the rows kept, in order.
const IN_ORDER: &str = "fetch_first::offset_and_fetch_first_keep_the_rows_in_order_7_17";
/// The test of the counts given or refused.
const COUNTS: &str = "fetch_first::a_count_is_a_value_given_or_refused_7_17";
/// The test of a recursive query of the clauses.
const RECURSION: &str = "fetch_first::a_recursive_query_of_the_clauses_7_17";
/// The test of a view and a cursor of the clauses.
const VIEWS: &str = "fetch_first::a_view_and_a_cursor_of_the_clauses_7_17";

/// The mutations of OFFSET and FETCH FIRST, in the order they run.
pub const LIMITS: [Mutation; 10] = [
    Mutation {
        name: "limit-offset-not-skipped",
        package: Package::Core,
        breaks: "OFFSET passes over no row [§7.17 GR6c]",
        file: RUN,
        original: "if row.is_ok() && self.skip > 0 {",
        replacement: "if row.is_ok() && self.skip > 0 && false {",
        integration: &[IN_ORDER],
        unit: &[KEPT],
    },
    Mutation {
        name: "limit-fetch-one-more",
        package: Package::Core,
        breaks: "FETCH FIRST keeps one row more than its count [§7.17 GR7d]",
        file: RUN,
        original: "let fetch = limit.fetch.as_ref().map(|e| count(e, subqueries, &Rule::FETCH)).transpose()?;",
        replacement: "let fetch = limit.fetch.as_ref().map(|e| count(e, subqueries, &Rule::FETCH)).transpose()?.map(|n: usize| n.saturating_add(1));",
        integration: &[IN_ORDER],
        unit: &[KEPT],
    },
    Mutation {
        name: "limit-null-taken",
        package: Package::Core,
        breaks: "a null row count raises no exception of its clause",
        file: RUN,
        original: "    if v.is_null() {",
        replacement: "    if v.is_null() && false {",
        integration: &[COUNTS],
        unit: &[REFUSED],
    },
    Mutation {
        name: "limit-offset-least",
        package: Package::Core,
        breaks: "an offset row count below 0 raises no exception [§7.17 GR6b]",
        file: RUN,
        original: "        least: 0,",
        replacement: "        least: -1,",
        integration: &[COUNTS],
        unit: &[REFUSED],
    },
    Mutation {
        name: "limit-recursive-fetch",
        package: Package::Core,
        breaks: "a recursive query of FETCH FIRST breaks SR3j ix 4 in place of ix 3",
        file: STRATA,
        original: "if e.query.ordered().is_some_and(|o| o.fetch.is_some()) {",
        replacement: "if e.query.ordered().is_some_and(|o| o.fetch.is_some() && false) {",
        integration: &[RECURSION],
        unit: &[],
    },
    Mutation {
        name: "limit-recursive-nested-fetch",
        package: Package::Core,
        breaks: "a query primary of FETCH FIRST referencing its stratum is taken [§7.17 SR3j ix 4]",
        file: STRATA,
        original: "                if o.fetch.is_some() {",
        replacement: "                if o.fetch.is_some() && false {",
        integration: &[RECURSION],
        unit: &[],
    },
    Mutation {
        name: "limit-fetch-updatable",
        package: Package::Core,
        breaks: "a query of FETCH FIRST is updatable [§7.17 SR7, SR11]",
        file: "src/ast/ordered.rs",
        original: "(o.offset.is_none() && o.fetch.is_none() && o.body.spec().is_some())",
        replacement: "(o.offset.is_none() && o.body.spec().is_some())",
        integration: &[VIEWS],
        unit: &[],
    },
    Mutation {
        name: "limit-distributes",
        package: Package::Core,
        breaks: "an OFFSET over a work table is found of the rows each iteration adds, not of all [§7.17 GR2c ii]",
        file: RECURSIVE,
        original: "LogicalPlan::Limit { .. } | LogicalPlan::Window(_) => false,",
        replacement: "LogicalPlan::Limit { input, .. } => distributes(input, distinct),\n        LogicalPlan::Window(_) => false,",
        integration: &[RECURSION],
        unit: &[],
    },
    Mutation {
        name: "limit-recursive-plan-lost",
        package: Package::Core,
        breaks: "a recursive query sorted or limited at its top has no plan to find its rows by",
        file: RECURSIVE,
        original: "else { return Fixpoint::Whole(vec![top]) };",
        replacement: "else { return Fixpoint::Whole(Vec::new()) };",
        integration: &[RECURSION],
        unit: &[],
    },
    Mutation {
        name: "limit-offset-correlation",
        package: Package::Core,
        breaks: "OFFSET before a count and ROWS is read as a correlation name",
        file: "src/parser/query.rs",
        original: "if self.at_partitioning() || self.at_window_clause() || self.at_result_offset() {",
        replacement: "if self.at_partitioning() || self.at_window_clause() {",
        integration: &[IN_ORDER],
        unit: &["parser::query::ordered::tests::the_clauses_where_a_query_expression_is_7_17"],
    },
];
