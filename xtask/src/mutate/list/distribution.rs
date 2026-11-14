//! The mutations of the inverse distribution functions (RULES.md
//! R-TEST-9): ROW0 and ROW1 swapped, PERCENTILE_DISC a row late, the
//! key's direction ignored, nulls kept by a grouping and by a window, a
//! fraction out of range taken, the empty group's fraction not looked at,
//! the fraction aggregated, and SR8d's window ordering taken.

use super::{Mutation, Package};

/// The test of each function against GR10's definitions.
const DEFINED: &str = "distribution::each_function_is_its_definition_10_9";
/// The test of nulls, empty groups and fractions.
const FRACTIONS: &str = "distribution::nulls_empty_groups_and_fractions_10_9";
/// The test of the refusals.
const REFUSED: &str = "distribution::inverse_distribution_functions_misused_are_refused_10_9";
/// The test over a window's partition.
const PARTITIONED: &str = "distribution::over_a_partition_10_9";

/// The mutations of the inverse distribution functions, in the order they
/// run.
pub const DISTRIBUTION: [Mutation; 9] = [
    Mutation {
        name: "cont-rows-swapped",
        package: Package::Core,
        breaks: "PERCENTILE_CONT interpolates from ROW1 toward ROW0, not from ROW0 toward ROW1 [§10.9 GR10h ii]",
        file: "src/functions/distribution.rs",
        original: "        return Ok(Some(Picked { low: whole.min(last), high: high.min(last), factor: part }));",
        replacement: "        return Ok(Some(Picked { low: high.min(last), high: whole.min(last), factor: part }));",
        integration: &[DEFINED],
        unit: &["functions::distribution::tests::cont_interpolates_between_its_rows_10_9"],
    },
    Mutation {
        name: "disc-a-row-late",
        package: Package::Core,
        breaks: "PERCENTILE_DISC takes the value after the first whose CUME_DIST is at least the fraction [§10.9 GR10h iii]",
        file: "src/functions/distribution.rs",
        original: "    let row = ceiling.saturating_sub(1).min(last);",
        replacement: "    let row = ceiling.min(last);",
        integration: &[DEFINED],
        unit: &["functions::distribution::tests::disc_picks_the_first_value_of_a_cume_dist_at_least_10_9"],
    },
    Mutation {
        name: "distribution-direction-ignored",
        package: Package::Core,
        breaks: "an inverse distribution function of a DESC key picks from its values ascending [§10.9 GR10d, §10.10 GR1c]",
        file: "src/binder/distribution.rs",
        original: "        let order = SortOrder::of(k.descending, k.nulls_first);",
        replacement: "        let order = SortOrder::of(false, k.nulls_first);",
        integration: &[DEFINED, PARTITIONED],
        unit: &[],
    },
    Mutation {
        name: "distribution-nulls-kept",
        package: Package::Core,
        breaks: "a grouping sorts and counts the null values of an inverse distribution function [§10.9 GR10d]",
        file: "src/exec/group/distribution.rs",
        original: "        if v.is_null() {\n            return Ok(());\n        }\n        pick.count",
        replacement: "        if v.is_null() && false {\n            return Ok(());\n        }\n        pick.count",
        integration: &[DEFINED],
        unit: &[],
    },
    Mutation {
        name: "distribution-window-nulls-kept",
        package: Package::Core,
        breaks: "a window sorts the null values of an inverse distribution function among its partition's [§10.9 GR10d]",
        file: "src/exec/window/distribution.rs",
        original: "                Some(Value::Null) => nulls = nulls || taken,",
        replacement: "                Some(Value::Null) => sorter.push(vec![Value::Null], &mut work)?,",
        integration: &[PARTITIONED],
        unit: &[],
    },
    Mutation {
        name: "fraction-out-of-range-taken",
        package: Package::Core,
        breaks: "a fraction below 0 or above 1 picks a value, not SQLSTATE 22003 [§10.9 GR10c]",
        file: "src/functions/distribution.rs",
        original: "    if !within {",
        replacement: "    if !within && false {",
        integration: &[FRACTIONS],
        unit: &["functions::distribution::tests::fractions_null_out_of_range_and_of_other_types_10_9"],
    },
    Mutation {
        name: "empty-group-fraction-unchecked",
        package: Package::Core,
        breaks: "a group of no values gives null whatever its fraction, not SQLSTATE 22003 of one out of range [§10.9 GR10c]",
        file: "src/exec/group/distribution.rs",
        original: "    let chosen = picked(d.continuous, &pick.fraction, pick.count)?;",
        replacement: "    let chosen = if pick.count == 0 { None } else { picked(d.continuous, &pick.fraction, pick.count)? };",
        integration: &[FRACTIONS],
        unit: &[],
    },
    Mutation {
        name: "distribution-fraction-aggregated",
        package: Package::Core,
        breaks: "the fraction of a grouped inverse distribution function may read a column of the rows, not of the groups [§10.9 SR14]",
        file: "src/binder/distribution.rs",
        original: "        self.fraction(sf.name, fraction, scope)?;",
        replacement: "        let _ = scope;",
        integration: &[REFUSED],
        unit: &[],
    },
    Mutation {
        name: "distribution-window-ordering-taken",
        package: Package::Core,
        breaks: "an inverse distribution function is taken over a window of an ordering or a frame [§9.23 SR8d i]",
        file: "src/binder/window/spec.rs",
        original: "    if sf.name == LISTAGG || matches!(sf.arg, SetArg::Fraction(_)) {",
        replacement: "    if sf.name == LISTAGG {",
        integration: &[REFUSED],
        unit: &[],
    },
];
