//! The mutations of a load's lanes parsed into their columns' types
//! (RULES.md R-TEST-9, D1148): an exact number written straight with a
//! whole digit more than the precision leaves, one written straight with
//! more fraction digits than the scale keeps, a string written straight
//! one character longer than its type, and a CHARACTER's record written
//! with its trailing spaces.

use super::super::{Mutation, Package};

/// The file of a lane's parse.
const PARSE: &str = "src/storage/lanes/parse.rs";
/// The unit test of integers parsed.
const INTEGERS: &str = "storage::lanes::parse::tests::integers_parse_as_rows_give_them";
/// The unit test of decimals parsed.
const DECIMALS: &str = "storage::lanes::parse::tests::decimals_parse_as_rows_give_them";
/// The unit test of a generated sweep parsed.
const SWEEP: &str = "storage::lanes::parse::tests::a_generated_sweep_parses_as_rows_give_it";
/// The unit test of strings parsed.
const STRINGS: &str = "storage::lanes::parse::tests::strings_parse_as_rows_give_them";

/// Mechanisms of a lane's parse whose removal must fail their tests.
pub const LANE_PARSE: [Mutation; 4] = [
    Mutation {
        name: "lane-parse-exact-range",
        package: Package::Core,
        breaks: "an exact number of a whole digit more than the precision leaves is written straight, not refused as the row is",
        file: PARSE,
        original: "if precision.checked_sub(scale).is_none_or(|room| n.whole > room) {",
        replacement: "if precision.checked_sub(scale).is_none_or(|room| n.whole > room.saturating_add(1)) {",
        integration: &[],
        unit: &[INTEGERS, DECIMALS, SWEEP],
    },
    Mutation {
        name: "lane-parse-fraction-fallback",
        package: Package::Core,
        breaks: "an exact number of more fraction digits than the scale is written straight, at the wrong scale, not truncated",
        file: PARSE,
        original: "if precision > MAX_EXACT_PRECISION || n.fraction > scale {",
        replacement: "if precision > MAX_EXACT_PRECISION {",
        integration: &[],
        unit: &[DECIMALS, SWEEP],
    },
    Mutation {
        name: "lane-parse-string-length",
        package: Package::Core,
        breaks: "a string one character longer than its type is written straight, not cut or refused as the row's cast does",
        file: PARSE,
        original: "units.count(text) <= most)",
        replacement: "units.count(text) <= most.saturating_add(1))",
        integration: &[],
        unit: &[STRINGS],
    },
    Mutation {
        name: "lane-parse-character-padding",
        package: Package::Core,
        breaks: "a CHARACTER's record is written with its trailing spaces, which the type's record drops",
        file: PARSE,
        original: "if varying { text } else { text.trim_end_matches(SPACE) }",
        replacement: "text",
        integration: &[],
        unit: &[STRINGS],
    },
];
