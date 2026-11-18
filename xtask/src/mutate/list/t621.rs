//! The mutations of Feature T621 (RULES.md R-TEST-9): the sample
//! variance's divisor, a pair with a null kept, REGR_R2 of Y alike,
//! Chan's merge of two states, POWER of a negative base, and FLOOR of a
//! negative exact number.

use super::{Mutation, Package};

/// The mutations of the statistical set functions and the numeric value
/// functions, in the order they run.
pub const T621: [Mutation; 6] = [
    Mutation {
        name: "sample-variance-over-n",
        package: Package::Core,
        breaks: "VAR_SAMP divides by N, not N − 1 [§10.9 GR7d ix 2 B]",
        file: "src/functions/statistics/moments.rs",
        original: "        (self.count > 1).then(|| s / (self.n - 1.0_f64))",
        replacement: "        (self.count > 1).then(|| s / self.n)",
        integration: &["statistical_set_functions::variances_of_values_worked_by_hand_10_9"],
        unit: &["functions::statistics::tests::each_of_values_worked_by_hand_10_9"],
    },
    Mutation {
        name: "pair-with-null-kept",
        package: Package::Core,
        breaks: "a pair of a null Y and an X is not eliminated [§10.9 GR8a]",
        file: "src/functions/statistics.rs",
        original: "        if args.iter().any(Value::is_null) { Ok(Value::Null) } else { Ok(Value::Row(args.into())) }",
        replacement: "        if args.iter().all(Value::is_null) { Ok(Value::Null) } else { Ok(Value::Row(args.into())) }",
        integration: &["statistical_set_functions::pairs_with_a_null_are_dropped_and_warned_10_9"],
        unit: &["functions::statistics::tests::pair_of_a_null_is_null_10_9"],
    },
    Mutation {
        name: "r2-of-y-alike-null",
        package: Package::Core,
        breaks: "REGR_R2 of Y all alike is null, not 1 [§10.9 GR8c xi 2]",
        file: "src/functions/statistics/moments.rs",
        original: "            Statistic::R2 => Some(if syy > 0.0 { (sxy / sxx * (sxy / syy)).min(1.0) } else { 1.0 }),",
        replacement: "            Statistic::R2 => (syy > 0.0).then(|| (sxy / sxx * (sxy / syy)).min(1.0)),",
        integration: &["statistical_set_functions::regr_r2_and_corr_of_values_alike_10_9"],
        unit: &["functions::statistics::tests::values_alike_10_9"],
    },
    Mutation {
        name: "merge-drops-spread",
        package: Package::Core,
        breaks: "Chan's merge leaves out the spread between the two states' means, so a window's frames merged lose it [RULES.md D53]",
        file: "src/functions/statistics/moments.rs",
        original: "        self.sxx += other.sxx + dx * dx * weight;",
        replacement: "        self.sxx += other.sxx;",
        integration: &["statistical_set_functions::window_functions_10_9"],
        unit: &["functions::statistics::tests::states_merge_10_9"],
    },
    Mutation {
        name: "power-negative-base-unsigned",
        package: Package::Core,
        breaks: "POWER of a negative base and an odd exponent is positive [§6.31 GR14g]",
        file: "src/functions/numeric.rs",
        original: "        finite(if odd { -raised } else { raised }, \"§6.31 GR14h\")?",
        replacement: "        finite(raised, \"§6.31 GR14h\")?",
        integration: &["numeric_functions::logarithms_powers_and_roots_6_31"],
        unit: &["functions::numeric::tests::power_cases_6_31"],
    },
    Mutation {
        name: "floor-truncates",
        package: Package::Core,
        breaks: "FLOOR of a negative exact number truncates toward 0, not down [§6.31 GR15b i]",
        file: "src/functions/numeric.rs",
        original: "units.checked_neg()?.checked_div_euclid(unit)?.checked_neg() } else { units.checked_div_euclid(unit) }",
        replacement: "units.checked_neg()?.checked_div_euclid(unit)?.checked_neg() } else { units.checked_div(unit) }",
        integration: &["numeric_functions::floor_and_ceiling_6_31"],
        unit: &["functions::numeric::tests::floor_and_ceiling_6_31"],
    },
];
