//! The mutations of the first rows FETCH FIRST asks for, read alone
//! (RULES.md R-TEST-9): a sort of only those giving up an earlier equal
//! row for a later, no peer, or no row given up as a peer; its rows past
//! the work area losing one held; the count asked for leaving OFFSET out;
//! a scan told of WITH TIES; a projection that can raise passed through;
//! an input that can raise not read to its end, or not asked to be; a column store's ordered
//! read keeping one row fewer.

use super::{Mutation, Package};

/// The file of a sort of the first rows.
const TOP: &str = "src/exec/sort/top.rs";
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
pub const BOUNDED_FETCH: [Mutation; 10] = [
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
];
