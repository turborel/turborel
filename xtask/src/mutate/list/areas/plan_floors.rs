//! The mutations of plans made with few statistics (RULES.md R-TEST-9,
//! D655, D657, D658): a table read's filter estimated below a row, `<=`
//! guessed as `<`, and a table opened with rows not due.

use super::super::{Mutation, Package};

/// The filter of a table's access path.
const FILTER_FILE: &str = "src/physical/access/filter.rs";

/// Mechanisms of plans made with few statistics whose removal must fail
/// their tests.
pub const PLAN_FLOORS: [Mutation; 3] = [
    Mutation {
        name: "plan-floors-filter-row",
        package: Package::Core,
        breaks: "a table read's filter of a small table is estimated to keep a fraction of a row, shown and costed as none",
        file: FILTER_FILE,
        original: "let est = at_least_one(with.read * kept, with.read).min(with.most);",
        replacement: "let est = (with.read * kept).min(with.most);",
        integration: &["tpch_plans::tpch_plans_hold_as_statistics_are_gathered"],
        unit: &["physical::access::filter::tests::a_filter_of_rows_keeps_at_least_one"],
    },
    Mutation {
        name: "plan-floors-guess-at-most",
        package: Package::Core,
        breaks: "with no statistics, `<=` and `>=` are guessed to keep what `<` and `>` keep",
        file: "src/physical/estimate.rs",
        original: "CompOp::Le | CompOp::Ge => (range + equal).min(1.0_f64),",
        replacement: "CompOp::Le | CompOp::Ge => range,",
        integration: &[],
        unit: &["physical::estimate::tests::guesses_without_statistics_are_consistent"],
    },
    Mutation {
        name: "plan-floors-due-at-open",
        package: Package::Core,
        breaks: "a table opened holding rows waits for a plan to ask for its statistics",
        file: "src/engine/rows/stats.rs",
        original: "        if rows > 0 {\n            self.asked.store(true, Ordering::Release);",
        replacement: "        if rows == usize::MAX {\n            self.asked.store(true, Ordering::Release);",
        integration: &["statistics::tables_opened_have_statistics"],
        unit: &[],
    },
];
