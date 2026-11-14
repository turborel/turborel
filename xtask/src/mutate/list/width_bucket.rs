//! The mutations of WIDTH_BUCKET (RULES.md R-TEST-9): the bound a
//! falling range is measured from, bound 2 itself inside the range, a
//! count not above 0 taken, equal bounds taken, and SR21's count type.

use super::{Mutation, Package};

/// The unit test of every bucket against GR17.
const GENERAL_RULES: &str = "functions::width_bucket::tests::buckets_are_general_rule_17_6_31";

/// The mutations of WIDTH_BUCKET, in the order they run.
pub const WIDTH_BUCKET: [Mutation; 5] = [
    Mutation {
        name: "falling-range-unmirrored",
        package: Package::Core,
        breaks: "a range whose bound 1 is the greater is measured from its lesser bound, so its buckets are not mirrored [§6.31 GR17e iii]",
        file: "src/functions/width_bucket.rs",
        original: "        let start = if self.rising { self.low } else { self.high };",
        replacement: "        let start = self.low;",
        integration: &["width_bucket::buckets_of_each_operand_6_31"],
        unit: &[GENERAL_RULES],
    },
    Mutation {
        name: "bound-2-inside",
        package: Package::Core,
        breaks: "an operand equal to bound 2 of a rising range is in bucket WBC, not WBC+1 [§6.31 GR17d ii]",
        file: "src/functions/width_bucket.rs",
        original: "            (to_low == Ordering::Less, to_high != Ordering::Less)",
        replacement: "            (to_low == Ordering::Less, to_high == Ordering::Greater)",
        integration: &["width_bucket::buckets_of_each_operand_6_31"],
        unit: &[GENERAL_RULES],
    },
    Mutation {
        name: "count-not-positive-taken",
        package: Package::Core,
        breaks: "a width bucket count of 0 or below is taken, not 2201G [§6.31 GR17b]",
        file: "src/functions/width_bucket.rs",
        original: "        if wbc <= 0 {",
        replacement: "        if wbc < 0 && wbc > 0 {",
        integration: &["width_bucket::failures_are_2201g_and_22003_6_31"],
        unit: &["functions::width_bucket::tests::failures_and_nulls_6_31"],
    },
    Mutation {
        name: "equal-bounds-taken",
        package: Package::Core,
        breaks: "equal width bucket bounds are taken as a falling range, not 2201G [§6.31 GR17c]",
        file: "src/functions/width_bucket.rs",
        original: "            Some(Ordering::Equal) => return invalid(\"[ISO/IEC 9075-2:2023 §6.31 GR17c] the width bucket bounds are equal\"),",
        replacement: "            Some(Ordering::Equal) => (bound_2, bound_1, false),",
        integration: &["width_bucket::failures_are_2201g_and_22003_6_31"],
        unit: &["functions::width_bucket::tests::failures_and_nulls_6_31"],
    },
    Mutation {
        name: "count-of-any-number",
        package: Package::Core,
        breaks: "a width bucket count of any numeric type is taken [§6.31 SR21]",
        file: "src/functions/width_bucket.rs",
        original: "        if !matches!(count, DataType::Exact { scale: 0, .. }) {",
        replacement: "        if !count.is_numeric() {",
        integration: &["width_bucket::types_are_syntax_rule_21_6_31"],
        unit: &["functions::width_bucket::tests::types_are_syntax_rule_21_6_31"],
    },
];
