//! The mutations of the hypothetical set functions (RULES.md R-TEST-9):
//! peers counted as before, the keys' directions ignored, CUME_DIST
//! without the hypothetical row, DENSE_RANK not distinct, PERCENT_RANK of
//! no rows 1, the values aggregated, the collation ignored, and SR8c's
//! window taken.

use super::{Mutation, Package};

/// The test of each function against GR9's window definition.
const DEFINED: &str = "hypothetical::each_function_is_its_window_definition_10_9";
/// The unit test of each function against its window function.
const WINDOWED: &str = "functions::hypothetical::tests::each_function_is_its_window_functions_value_10_9";

/// The mutations of the hypothetical set functions, in the order they run.
pub const HYPOTHETICAL: [Mutation; 8] = [
    Mutation {
        name: "hypothetical-peers-before",
        package: Package::Core,
        breaks: "a row that is a peer of the hypothetical row is counted before it [§10.9 GR9, §9.23 SR5e]",
        file: "src/functions/hypothetical.rs",
        original: "            Ordering::Equal => self.peers = self.peers.saturating_add(1),",
        replacement: "            Ordering::Equal => self.before = self.before.saturating_add(1),",
        integration: &[DEFINED],
        unit: &[WINDOWED],
    },
    Mutation {
        name: "hypothetical-directions-ignored",
        package: Package::Core,
        breaks: "a sort key of DESC orders the hypothetical row ascending [§10.9 GR9, §10.10 GR1c]",
        file: "src/binder/hypothetical.rs",
        original: "        let orders = sf.order_by.iter().map(|k| SortOrder::of(k.descending, k.nulls_first)).collect();",
        replacement: "        let orders = sf.order_by.iter().map(|k| SortOrder::of(false, k.nulls_first)).collect();",
        integration: &[DEFINED],
        unit: &[],
    },
    Mutation {
        name: "cume-dist-without-hypothetical-row",
        package: Package::Core,
        breaks: "CUME_DIST counts the group's rows alone, not the hypothetical row among them [§10.9 GR9, §9.23 SR5h]",
        file: "src/functions/hypothetical.rs",
        original: "            Hypothesis::CumeDist => Value::Approx(real(rank.saturating_add(self.peers)) / real(self.rows.saturating_add(1))),",
        replacement: "            Hypothesis::CumeDist => Value::Approx(real(self.before.saturating_add(self.peers)) / real(self.rows.max(1))),",
        integration: &[DEFINED],
        unit: &[WINDOWED],
    },
    Mutation {
        name: "dense-rank-not-distinct",
        package: Package::Core,
        breaks: "DENSE_RANK counts every row before the hypothetical row, not each distinct one once [§10.9 GR9, §9.23 SR5e]",
        file: "src/binder/hypothetical.rs",
        original: "        let distinct = hypothesis == Hypothesis::DenseRank;",
        replacement: "        let distinct = false;",
        integration: &[DEFINED, "hypothetical::hypothetical_set_functions_past_the_work_area_10_9"],
        unit: &[],
    },
    Mutation {
        name: "percent-rank-of-no-rows-1",
        package: Package::Core,
        breaks: "PERCENT_RANK of a group of no rows is 1, not 0, of the hypothetical row alone [§10.9 GR9, §9.23 SR5g]",
        file: "src/functions/hypothetical.rs",
        original: "            Hypothesis::PercentRank => Value::Approx(if self.rows == 0 { 0.0_f64 } else { real(self.before) / real(self.rows) }),",
        replacement: "            Hypothesis::PercentRank => Value::Approx(if self.rows == 0 { 1.0_f64 } else { real(self.before) / real(self.rows) }),",
        integration: &["hypothetical::empty_groups_grouping_sets_parameters_and_views_10_9"],
        unit: &["functions::hypothetical::tests::no_rows_rank_first_10_9"],
    },
    Mutation {
        name: "hypothetical-values-aggregated",
        package: Package::Core,
        breaks: "a value of a hypothetical set function names a column not grouped [§10.9 SR14]",
        file: "src/binder/hypothetical.rs",
        original: "        self.typed(v, taken(), scope)?;",
        replacement: "        self.typed(v, taken(), rows)?;",
        integration: &["hypothetical::values_are_of_the_groups_10_9"],
        unit: &[],
    },
    Mutation {
        name: "hypothetical-collation-ignored",
        package: Package::Core,
        breaks: "character strings are compared by their characters' codes, not the collation of value and key [§10.9 SR10c i]",
        file: "src/binder/hypothetical.rs",
        original: "        let collation = if common.is_character() { self.collation_of(&[self.derived(v)?, self.derived(&k.key)?])? } else { None };",
        replacement: "        let collation = None::<crate::value::collation::CollationRef>;",
        integration: &["hypothetical::several_keys_collations_and_types_10_9"],
        unit: &[],
    },
    Mutation {
        name: "hypothetical-over-a-window",
        package: Package::Core,
        breaks: "a hypothetical set function is taken over a window [§9.23 SR8c]",
        file: "src/binder/window/spec.rs",
        original: "    if matches!(sf.arg, SetArg::Hypothetical(_)) {",
        replacement: "    if false && matches!(sf.arg, SetArg::Hypothetical(_)) {",
        integration: &["hypothetical::hypothetical_set_functions_misused_are_refused_10_9"],
        unit: &[],
    },
];
