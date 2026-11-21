//! The mutations of OFFSET and FETCH FIRST (RULES.md R-TEST-9): rows not
//! passed over, one more kept, a null count or one below its least taken,
//! SR3j ix 3 and ix 4 dropped, a query of OFFSET or FETCH FIRST updatable,
//! an OFFSET over a work table found of the rows an iteration adds, a
//! recursive query of ORDER BY losing its plan, OFFSET read as a
//! correlation name before its count; of WITH TIES no peer kept, or every
//! row a peer, or no ORDER BY taken; of PERCENT a count of the rows after
//! OFFSET, a floor for a ceiling, a DECFLOAT's ceiling inexact, a null
//! percentage or one not a number taken; of a path in the order asked for
//! below FETCH FIRST, the rows it reads unpriced, or OFFSET's left out, or
//! known through a filter, or a path read whole priced by them.

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
/// The file of a percentage's count.
const PERCENT: &str = "src/exec/limit/percent.rs";
/// The file of OFFSET and FETCH FIRST bound.
const BOUND: &str = "src/binder/limit.rs";
/// The unit test of WITH TIES.
const TIES_KEPT: &str = "exec::limit::tests::with_ties_keeps_the_peers_of_the_last_row_7_17";
/// The unit test of a percentage.
const PERCENT_KEPT: &str = "exec::limit::tests::a_percentage_keeps_its_ceiling_of_the_rows_7_17";
/// The file of access paths.
const ACCESS: &str = "src/physical/access.rs";
/// The test of a path in order priced by the rows it reads.
const PRICED: &str = "fetch_first::fetch_first_prices_a_path_in_order_by_the_rows_it_reads";
/// The test of WITH TIES.
const TIES: &str = "fetch_ties::with_ties_keeps_the_peers_of_the_last_row_7_17";
/// The test of a percentage.
const PERCENTAGE: &str = "fetch_ties::a_percentage_keeps_its_ceiling_of_the_rows_7_17";
/// The test of the refusals of PERCENT and WITH TIES.
const OPTIONS_REFUSED: &str = "fetch_ties::the_options_refused_7_17";

/// The mutations of OFFSET and FETCH FIRST, in the order they run.
pub const LIMITS: [Mutation; 22] = [
    Mutation {
        name: "limit-offset-not-skipped",
        package: Package::Core,
        breaks: "OFFSET passes over no row [§7.17 GR6c]",
        file: RUN,
        original: "            if self.skip > 0 {",
        replacement: "            if self.skip > 0 && false {",
        integration: &[IN_ORDER],
        unit: &[KEPT],
    },
    Mutation {
        name: "limit-fetch-one-more",
        package: Package::Core,
        breaks: "FETCH FIRST keeps one row more than its count [§7.17 GR7d]",
        file: RUN,
        original: "let left = count(&f.count, subqueries, &Rule::FETCH)?;",
        replacement: "let left = count(&f.count, subqueries, &Rule::FETCH)?.saturating_add(1);",
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
        original: "LogicalPlan::Limit { .. } | LogicalPlan::Window(_) | LogicalPlan::Recognize(_) | LogicalPlan::Polymorphic(_) => false,",
        replacement: "LogicalPlan::Limit { input, .. } => distributes(input, distinct),\n        LogicalPlan::Window(_) | LogicalPlan::Recognize(_) | LogicalPlan::Polymorphic(_) => false,",
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
        original: "            || self.at_result_offset()\n            || self.at_match_recognize()",
        replacement: "            || self.at_match_recognize()",
        integration: &[IN_ORDER],
        unit: &["parser::query::ordered::tests::the_clauses_where_a_query_expression_is_7_17"],
    },
    Mutation {
        name: "limit-ties-no-peer",
        package: Package::Core,
        breaks: "WITH TIES keeps no peer of the last row [§7.17 GR7d i]",
        file: RUN,
        original: "if compare_rows(&ties.keys, last, next) == Ordering::Equal {",
        replacement: "if compare_rows(&ties.keys, last, next) == Ordering::Equal && false {",
        integration: &[TIES],
        unit: &[TIES_KEPT],
    },
    Mutation {
        name: "limit-ties-keys-lost",
        package: Package::Core,
        breaks: "WITH TIES compares rows by no key, so every row after is a peer [§7.17 GR7d i]",
        file: "src/binder/order_by.rs",
        original: "peers.clone_from(&sorted);",
        replacement: "peers.clear();",
        integration: &[TIES],
        unit: &[],
    },
    Mutation {
        name: "limit-ties-unordered",
        package: Package::Core,
        breaks: "WITH TIES of a query expression of no ORDER BY is taken [§7.17 SR30]",
        file: BOUND,
        original: "if f.ties && o.order_by.is_empty() {",
        replacement: "if f.ties && o.order_by.is_empty() && false {",
        integration: &[OPTIONS_REFUSED],
        unit: &[],
    },
    Mutation {
        name: "limit-percent-after-offset",
        package: Package::Core,
        breaks: "a percentage is of the rows after OFFSET, not before [§7.17 NOTE 328]",
        file: RUN,
        original: "let left = percent::count(&f.count, held.len(), subqueries)?;",
        replacement: "let left = percent::count(&f.count, held.len().saturating_sub(skip), subqueries)?;",
        integration: &[PERCENTAGE],
        unit: &[PERCENT_KEPT],
    },
    Mutation {
        name: "limit-percent-floor",
        package: Package::Core,
        breaks: "a percentage keeps the floor of its rows, not the ceiling [§7.17 GR7b ii]",
        file: PERCENT,
        original: "format!(\"{:.0}\", x.ceil())",
        replacement: "format!(\"{:.0}\", x.floor())",
        integration: &[PERCENTAGE],
        unit: &[PERCENT_KEPT],
    },
    Mutation {
        name: "limit-percent-decfloat-floor",
        package: Package::Core,
        breaks: "a DECFLOAT percentage keeps the floor of its rows, not the ceiling [§7.17 GR7b ii]",
        file: PERCENT,
        original: "c.div_euclid(p).saturating_add(i128::from(c.rem_euclid(p) > 0))",
        replacement: "c.div_euclid(p)",
        integration: &[],
        unit: &[PERCENT_KEPT],
    },
    Mutation {
        name: "limit-percent-null",
        package: Package::Core,
        breaks: "a null percentage raises no exception of FETCH FIRST",
        file: PERCENT,
        original: "    if percentage.is_null() {",
        replacement: "    if percentage.is_null() && false {",
        integration: &[OPTIONS_REFUSED],
        unit: &[PERCENT_KEPT],
    },
    Mutation {
        name: "limit-percent-not-a-number",
        package: Package::Core,
        breaks: "a percentage not a number is taken [§7.17 SR29]",
        file: BOUND,
        original: "    if !t.is_numeric() {",
        replacement: "    if !t.is_numeric() && false {",
        integration: &[OPTIONS_REFUSED],
        unit: &[],
    },
    Mutation {
        name: "limit-first-unpriced",
        package: Package::Core,
        breaks: "a path in order below FETCH FIRST is priced by all its rows, so the table is sorted",
        file: ACCESS,
        original: "        let asked = self.first.filter(|_| !want.is_empty() && !tied);",
        replacement: "        let asked = self.first.filter(|_| false);",
        integration: &[PRICED],
        unit: &[],
    },
    Mutation {
        name: "limit-first-offset-left-out",
        package: Package::Core,
        breaks: "the rows OFFSET passes over are not counted among those read",
        file: "src/logical/limit.rs",
        original: "        passed?.checked_add(self.counted()?)",
        replacement: "        passed.map(|_| 0_usize)?.checked_add(self.counted()?)",
        integration: &[PRICED],
        unit: &[],
    },
    Mutation {
        name: "limit-first-through-a-filter",
        package: Package::Core,
        breaks: "a filter divides the first rows read below it by the share of rows it keeps",
        file: "src/physical.rs",
        original: "        let goal = self.divided(|| kept(self));",
        replacement: "        let goal = *self;",
        integration: &[PRICED],
        unit: &[],
    },
    Mutation {
        name: "limit-first-read-whole",
        package: Package::Core,
        breaks: "a scan read whole to be sorted is priced by the rows FETCH FIRST asks",
        file: ACCESS,
        original: "        let total = if scan.ordered { scan.total * self.read_share(want, scan.est) } else { scan.total };",
        replacement: "        let total = scan.total * self.read_share(want, scan.est);",
        integration: &[PRICED],
        unit: &[],
    },
];
