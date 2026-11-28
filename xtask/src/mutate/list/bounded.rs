//! The mutations of the first rows FETCH FIRST asks for, read alone
//! (RULES.md R-TEST-9): a sort of only those giving up an earlier equal
//! row for a later, no peer, or no row given up as a peer; its rows past
//! the work area losing one held; the count asked for leaving OFFSET out;
//! a scan told of WITH TIES; a projection that can raise passed through;
//! an input that can raise not read to its end, or not asked to be; a column store's ordered
//! read keeping one row fewer; and the bound of a sort's first key judged
//! of each row by the scan, a sort of its first rows run as a part, its
//! morsels' late columns and the bound they share (D1241).

use super::{Mutation, Package};

/// The file of a sort of the first rows.
const TOP: &str = "src/exec/sort/top.rs";
/// The test of a scan judging a bound of each row, and of a top-N run as a part.
const TOP_PART: &str = "topped_sorts::a_scan_judges_a_top_bound_of_each_row_and_a_top_is_a_part";
/// The file of the rows asked for, told the input.
const BOUNDED: &str = "src/exec/limit/bounded.rs";
/// The file of OFFSET and FETCH FIRST run.
const RUN: &str = "src/exec/limit.rs";
/// The unit test of a sort of the first rows.
const TOP_KEPT: &str = "exec::sort::top::tests::the_first_rows_held_are_a_whole_sorts_7_17";
/// The unit test of a sort of the first rows past the work area.
const TOP_WHOLE: &str = "exec::sort::top::tests::rows_held_past_the_work_area_are_sorted_whole";
/// The unit test of the rows asked for.
const ASKED: &str = "exec::limit::tests::the_input_is_told_the_rows_asked_for";
/// The unit test of an input read to its end.
const TO_ITS_END: &str = "exec::limit::tests::an_input_that_can_raise_is_read_to_its_end_7_17";
/// The test of a sort of the first rows.
const SORTED: &str = "fetch_bounded::a_sort_of_the_first_rows_gives_a_whole_sorts_7_17";
/// The test of a column store's ordered scan.
const SCANNED: &str = "fetch_bounded::an_ordered_scan_of_the_column_store_reads_the_first_rows_7_17";
/// The test of rows that can raise.
const RAISING: &str = "fetch_bounded::rows_that_can_raise_are_read_to_their_end_7_17";

/// See the module doc.
pub const BOUNDED_FETCH: [Mutation; 17] = [
    Mutation {
        name: "top-earlier-equal-given-up",
        package: Package::Core,
        breaks: "a sort of the first rows gives up an earlier equal row for a later, not as the whole sort",
        file: TOP,
        original: ".then(self.place.cmp(&other.place))",
        replacement: ".then(other.place.cmp(&self.place))",
        integration: &[SORTED],
        unit: &[TOP_KEPT],
    },
    Mutation {
        name: "top-no-peer",
        package: Package::Core,
        breaks: "a sort of the first rows of WITH TIES holds no peer of the greatest that came after it",
        file: TOP,
        original: "self.peers.push(held);",
        replacement: "drop(held);",
        integration: &[SORTED],
        unit: &[TOP_KEPT],
    },
    Mutation {
        name: "top-given-up-peer",
        package: Package::Core,
        breaks: "a sort of the first rows of WITH TIES lets go a row given up that is a peer of the greatest",
        file: TOP,
        original: "let peer = self.most.ties && self.heap.peek()",
        replacement: "let peer = false && self.heap.peek()",
        integration: &[SORTED],
        unit: &[TOP_KEPT],
    },
    Mutation {
        name: "top-whole-loses-a-row",
        package: Package::Core,
        breaks: "a sort of the first rows past the work area loses a row it held",
        file: TOP,
        original: "    for held in top.in_order_come() {",
        replacement: "    for held in top.in_order_come().into_iter().skip(1) {",
        integration: &[SORTED],
        unit: &[TOP_WHOLE],
    },
    Mutation {
        name: "bounded-without-offset",
        package: Package::Core,
        breaks: "the rows asked for of the input leave out those OFFSET passes over",
        file: RUN,
        original: "let most = skip.checked_add(left).map(",
        replacement: "let most = Some(left).map(",
        integration: &[SORTED],
        unit: &[ASKED],
    },
    Mutation {
        name: "bounded-scan-told-ties",
        package: Package::Core,
        breaks: "a scan in an order is told the rows asked for of WITH TIES, whose peers it may not read",
        file: BOUNDED,
        original: "if order.is_empty() || most.ties { before }",
        replacement: "if order.is_empty() { before }",
        integration: &[SCANNED],
        unit: &[],
    },
    Mutation {
        name: "bounded-through-raising-projection",
        package: Package::Core,
        breaks: "the rows asked for pass through a projection that can raise, so a row it would raise for is never computed",
        file: BOUNDED,
        original: "if exprs.iter().any(Expr::can_raise) { input }",
        replacement: "if false { input }",
        integration: &[RAISING],
        unit: &[],
    },
    Mutation {
        name: "limit-not-drained",
        package: Package::Core,
        breaks: "rows past those kept are not read, though asked to be [SQL-92 §3.3.4.4]",
        file: RUN,
        original: "        if !mem::take(&mut self.drain) {",
        replacement: "        if !mem::take(&mut self.drain) || true {",
        integration: &[RAISING],
        unit: &[TO_ITS_END],
    },
    Mutation {
        name: "limit-drain-not-asked",
        package: Package::Core,
        breaks: "an input that can raise is not read past the rows kept [SQL-92 §3.3.4.4]",
        file: RUN,
        original: "    let drain = input.can_raise();",
        replacement: "    let drain = input.can_raise() && false;",
        integration: &[RAISING],
        unit: &[],
    },
    Mutation {
        name: "ordered-read-keeps-one-fewer",
        package: Package::Core,
        breaks: "a column store's ordered read keeps one row fewer than those asked for",
        file: "src/engine/rows/frozen/ordered.rs",
        original: "            keep.truncate(most);",
        replacement: "            keep.truncate(most.saturating_sub(1));",
        integration: &[SCANNED],
        unit: &[],
    },
    Mutation {
        name: "top-bound-not-judged-a-row",
        package: Package::Core,
        breaks: "a scan passes a sort's bound over groups and parts alone, judging no row of it (RULES.md D1241)",
        file: "src/engine/columns/judged.rs",
        original: "        Conjunct::Dynamic { .. } | Conjunct::Like { .. } | Conjunct::Among { .. } => true,",
        replacement: "        Conjunct::Like { .. } | Conjunct::Among { .. } => true,\n        Conjunct::Dynamic { .. } => false,",
        integration: &[TOP_PART],
        unit: &["engine::columns::judged::tests::a_sorts_bound_is_judged_of_each_row"],
    },
    Mutation {
        name: "top-bound-keeps-rows-past-it",
        package: Package::Core,
        breaks: "a scan judging a sort's bound keeps the rows past it and leaves out those before it (RULES.md D1241)",
        file: "src/storage/condition/threshold.rs",
        original: "let op = if self.descending { CompOp::Ge } else { CompOp::Le };",
        replacement: "let op = if self.descending { CompOp::Le } else { CompOp::Ge };",
        integration: &[],
        unit: &["storage::condition::threshold::tests::a_threshold_is_a_test_of_a_row_once_set"],
    },
    Mutation {
        name: "top-of-column-store-serial",
        package: Package::Core,
        breaks: "a sort asked for its first rows over a column store table is no part (RULES.md D1241)",
        file: "src/physical/parallel.rs",
        original: "        let topped = self.first && matches!(plan, Plan::Sort { .. });",
        replacement: "        let topped = false;",
        integration: &[TOP_PART],
        unit: &[],
    },
    Mutation {
        name: "top-part-of-any-fetch",
        package: Package::Core,
        breaks: "a sort under FETCH FIRST WITH TIES or of a percentage, told no count of rows, is marked a part (RULES.md D1241)",
        file: "src/physical/parallel.rs",
        original: "limit.fetch.as_ref().is_some_and(|f| !f.percent && f.ties.is_none())",
        replacement: "limit.fetch.is_some()",
        integration: &[TOP_PART],
        unit: &[],
    },
    Mutation {
        name: "morsel-top-decodes-every-row",
        package: Package::Core,
        breaks: "a morsel read by a sort of its first rows decodes its late columns of every row its steps kept (RULES.md D1241)",
        file: "src/exec/parallel/batched.rs",
        original: "        by.unresolved = late;",
        replacement: "        by.unresolved = false;",
        integration: &[],
        unit: &["exec::parallel::batched::tests::a_sort_of_first_rows_leaves_its_other_columns_late_and_tells_its_bound"],
    },
    Mutation {
        name: "shared-bound-keeps-the-last",
        package: Package::Core,
        breaks: "a bound shared by a sort's morsels counts the last first keys offered, not the first (RULES.md D1241)",
        file: "src/storage/condition/threshold.rs",
        original: "is_some_and(|last| offered < *last)",
        replacement: "is_some_and(|last| offered > *last)",
        integration: &[],
        unit: &["storage::condition::threshold::tests::a_shared_threshold_is_the_last_of_the_first_offered"],
    },
    Mutation {
        name: "bounded-run-decodes-late-columns",
        package: Package::Core,
        breaks: "a run given whole under a sort's bound decodes its late columns of every row (RULES.md D1241)",
        file: "src/engine/rows/frozen/batches/run.rs",
        original: "(judged.decoded, self.judgment.bounded())",
        replacement: "(judged.decoded, false)",
        integration: &[],
        unit: &["engine::rows::frozen::tests::morsels::a_run_given_whole_under_a_bound_keeps_its_late_column_late"],
    },
];
