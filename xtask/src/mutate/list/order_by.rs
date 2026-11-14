//! The mutations of sort keys that are value expressions (RULES.md
//! R-TEST-9): the columns added for the sort left in the result, a key
//! alike a select item not sorted by its column, SR26b's set function and
//! partition column misjudged, SR26 d i 9's refusals dropped, a result's
//! name not its column, and an updatable cursor's keys not computed or
//! their columns not followed.

use super::{Mutation, Package};

/// The file of the binder's sort keys.
const KEYS: &str = "src/binder/order_by.rs";
/// The unit test of a key's rules.
const KEY_RULES: &str = "binder::order_by::tests::a_key_has_a_column_and_nothing_refused_7_17";
/// The test of a column left out.
const LEFT_OUT: &str = "order_by::a_column_left_out_sorts_the_rows_7_17";
/// The test of a key alike a select item.
const ALIKE: &str = "order_by::a_key_alike_a_select_item_sorts_by_its_column_7_17";
/// The test of a result's names in a key.
const NAMES: &str = "order_by::a_name_of_the_result_is_its_column_in_a_key_7_17";
/// The test of window functions as keys.
const WINDOWS: &str = "order_by::a_window_function_sorts_the_rows_6_10";
/// The test of the refusals.
const REFUSED: &str = "order_by::a_key_breaking_a_rule_is_refused_7_17";
/// The test of an updatable cursor ordered by an expression.
const CURSOR: &str = "order_by::an_updatable_cursor_is_ordered_by_its_keys_13_9";

/// The mutations of sort keys, in the order they run.
pub const ORDER_BY: [Mutation; 9] = [
    Mutation {
        name: "order-by-added-left-in",
        package: Package::Core,
        breaks: "the columns a sort adds are not taken away after it, so a row is wider than the result [§7.17 GR5]",
        file: KEYS,
        original: "        let kept = (0..visible).map(Expr::Column).collect();",
        replacement: "        let kept = (0..exprs.len()).map(Expr::Column).collect();",
        integration: &[LEFT_OUT, ALIKE],
        unit: &[],
    },
    Mutation {
        name: "order-by-alike-not-reused",
        package: Package::Core,
        breaks: "a key alike an item of the select list is computed again as a column of its own [§7.17 SR26 d i 6]",
        file: KEYS,
        original: "Placed::Hidden(n) => match added.get(n).and_then(|a| own.iter().position(|o| alike(o, a))) {",
        replacement: "Placed::Hidden(n) => match added.get(n).and_then(|a| own.iter().position(|o| alike(o, a) && false)) {",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "order-by-set-function-taken",
        package: Package::Core,
        breaks: "a sort key of a set function is taken [§7.17 SR26b]",
        file: KEYS,
        original: "    if v.any(&|e| matches!(e, ValueExpression::SetFunction(_))) {",
        replacement: "    if false {",
        integration: &[],
        unit: &[KEY_RULES],
    },
    Mutation {
        name: "order-by-partition-column-not-counted",
        package: Package::Core,
        breaks: "a window function whose one column reference is of its PARTITION BY is refused as a key of none [§7.17 SR26b]",
        file: KEYS,
        original: "    if refs.is_empty() && !v.any(&partitioned) {",
        replacement: "    if refs.is_empty() {",
        integration: &[WINDOWS],
        unit: &[KEY_RULES],
    },
    Mutation {
        name: "order-by-distinct-added",
        package: Package::Core,
        breaks: "SELECT DISTINCT is sorted by a column its select list leaves out [§7.17 SR26 d i 9 B II]",
        file: KEYS,
        original: "    if distinct {",
        replacement: "    if false {",
        integration: &[REFUSED],
        unit: &[],
    },
    Mutation {
        name: "order-by-grouped-not-f868",
        package: Package::Core,
        breaks: "a grouped query sorted by a column its select list leaves out is not told of as Feature F868 [§7.17 SR26 d i 9 B I]",
        file: KEYS,
        original: "    if grouped {",
        replacement: "    if false {",
        integration: &[REFUSED],
        unit: &[],
    },
    Mutation {
        name: "order-by-result-name-not-its-column",
        package: Package::Core,
        breaks: "in a key with a column left out, a name of a result column is a column of the table, not the select item [§7.17 SR26 d i 6]",
        file: KEYS,
        original: "        names.on = names.from.is_some_and(|from| i >= from);",
        replacement: "        names.on = false;",
        integration: &[NAMES],
        unit: &[],
    },
    Mutation {
        name: "order-by-cursor-key-not-computed",
        package: Package::Core,
        breaks: "an updatable cursor sorts its rows by the columns of the table at its keys' places, not by its keys [§14.3, Feature F833]",
        file: "src/db/scroll.rs",
        original: "keyed(&projected(row, &layers, &runner)?, &columns, id)",
        replacement: "keyed(&row, &columns, id)",
        integration: &[CURSOR],
        unit: &[],
    },
    Mutation {
        name: "order-by-cursor-key-columns-not-followed",
        package: Package::Core,
        breaks: "a positioned UPDATE may set a column a sort key's expression reads [ISO/IEC 9075:1992 §13.9 SR4]",
        file: "src/db/cursor/ordered.rs",
        original: "                columns = read;",
        replacement: "                let _ = read;",
        integration: &[CURSOR],
        unit: &[],
    },
];
