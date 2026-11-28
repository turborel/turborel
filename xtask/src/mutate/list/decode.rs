//! The mutations of the decode kernels (RULES.md R-TEST-9, D352-D353): a
//! block's value that runs past its word given the wrong rest, ALP's
//! header bit not written, a decimal kept that its factors do not give
//! back, an n made an `f64` off by one ulp, the cache of opened chunks
//! keyed by zone alone, not column too, and a chunk opened never found again;
//! and of 16-byte fields read as integers (D1287): a field's high half
//! not tested in the loop, one field cut to 64 bits, a constant past 64
//! bits read as 0, the chunks' bounds not tested, the chunks before a run
//! or of no position tested with its own, and a wide column's conjunct
//! not the scan's; and of a wide column's fields narrowed as its group is
//! written (D1285): a value past 64 bits cut to them, and a column whose
//! values fit left in 16 bytes.

use super::super::{Mutation, Package};

/// The routine of each width.
const BLOCKS_FILE: &str = "src/engine/columns/packed/blocks.rs";
/// ALP's encoder.
const ALP_FILE: &str = "src/engine/columns/alp.rs";
/// ALP's decoding kernel's portable body.
const ALP_DECODE_FILE: &str = "src/kernels/portable/alp.rs";
/// The chunks a scan opens once.
const PREPARED_FILE: &str = "src/engine/columns/prepared.rs";
/// The unit test of every width's blocks.
const BLOCKS: &str = "engine::columns::packed::blocks::tests::blocks_read_as_one_at_a_time";
/// The unit test of chunks of decimals read back.
const DECIMALS: &str = "engine::columns::alp::tests::chunks_of_decimals_read_back_bit_for_bit";
/// The unit test of the cache over runs.
const OPENED_ONCE: &str = "engine::columns::prepared::tests::chunks_are_opened_once_for_their_runs";

/// A chunk's values as integers.
const WORDS_FILE: &str = "src/engine/columns/encoded/words.rs";
/// The unit test of what a chunk of 16-byte fields refuses.
const PAST: &str = "engine::columns::tests::wide::a_value_past_64_bits_is_not_read_as_an_integer";
/// The unit test of a wide column's runs.
const WIDE_RUNS: &str = "engine::columns::prepared::tests::wide::a_wide_column_reads_as_integers_where_64_bits_hold_its_values";
/// The unit test of a wide column's positions.
const WIDE_PLACES: &str = "engine::columns::prepared::tests::wide::a_wide_column_reads_as_integers_at_positions";

/// A group's column as it is written.
const FIELDS_FILE: &str = "src/engine/columns/fields.rs";

/// Mechanisms of the decode kernels whose removal must fail their tests.
pub const DECODE: [Mutation; 15] = [
    Mutation {
        name: "decode-block-spill-shift",
        package: Package::Core,
        breaks: "a value whose bits run past its word takes the next word's bits one place off",
        file: BLOCKS_FILE,
        original: ".wrapping_shl(u64::BITS.wrapping_sub(shift))",
        replacement: ".wrapping_shl(u64::BITS.wrapping_sub(shift).wrapping_sub(1))",
        integration: &[],
        unit: &[BLOCKS, "engine::columns::packed::blocks::tests::blocks_sweep_of_fixed_seeds"],
    },
    Mutation {
        name: "decode-alp-header-bit",
        package: Package::Core,
        breaks: "an ALP chunk is written without its factors' bit, so it is read by division and its f taken for a count",
        file: ALP_FILE,
        original: "    out.push(factors.e | FACTORED);",
        replacement: "    out.push(factors.e);",
        integration: &[],
        unit: &[DECIMALS, "engine::columns::alp::tests::decimals_sweep_of_fixed_seeds"],
    },
    Mutation {
        name: "decode-alp-unverified",
        package: Package::Core,
        breaks: "a value is kept as a decimal though its factors' multiplications do not give it back",
        file: ALP_FILE,
        original: "    (factors.scale()?.of(exact(n)?).to_bits() == v.to_bits()).then_some(n)",
        replacement: "    factors.scale().map(|_| n)",
        integration: &[],
        unit: &["engine::columns::alp::tests::numbers_are_decimals_exactly", DECIMALS],
    },
    Mutation {
        name: "decode-alp-magic-off",
        package: Package::Core,
        breaks: "an n made an f64 by the magic number is one ulp off",
        file: ALP_DECODE_FILE,
        original: "f64::from_bits(MAGIC_BITS.wrapping_add_signed(*w)) - MAGIC",
        replacement: "f64::from_bits(MAGIC_BITS.wrapping_add_signed(*w).wrapping_add(1)) - MAGIC",
        integration: &[],
        unit: &[DECIMALS],
    },
    Mutation {
        name: "decode-cache-key-zone-only",
        package: Package::Core,
        breaks: "the chunks opened are kept by zone alone, so a column reads another's chunk of the same first row",
        file: PREPARED_FILE,
        original: "let kept = columns.get_mut(column)",
        replacement: "let kept = columns.get_mut(0)",
        integration: &[],
        unit: &[OPENED_ONCE, "engine::columns::prepared::tests::positions_read_through_the_cache"],
    },
    Mutation {
        name: "decode-cache-never-kept",
        package: Package::Core,
        breaks: "a chunk opened is never found again, so each run opens and parses it anew",
        file: PREPARED_FILE,
        original: "if let Some(at) = kept.iter().position(|h| h.first == first) {",
        replacement: "if let Some(at) = kept.iter().position(|h| h.first == first && h.end < first) {",
        integration: &[],
        unit: &[OPENED_ONCE],
    },
    Mutation {
        name: "decode-wide-fields-cut",
        package: Package::Core,
        breaks: "fixed fields of 16 bytes are integers only if each one's high half is its sign alone",
        file: WORDS_FILE,
        original: "        fit &= i64::from_le_bytes(*high) == units.wrapping_shr(SIGN_SHIFT);",
        replacement: "        fit &= high.len() == FRAMED;",
        integration: &[],
        unit: &[PAST],
    },
    Mutation {
        name: "decode-wide-field-cut",
        package: Package::Core,
        breaks: "a field of 16 bytes read alone, or a dictionary's entry, is an integer only if 64 bits hold it",
        file: WORDS_FILE,
        original: "        Ok(wide) => i64::try_from(i128::from_le_bytes(wide)).ok(),",
        replacement: "        Ok(wide) => wide.first_chunk::<FRAMED>().map(|low| i64::from_le_bytes(*low)),",
        integration: &[],
        unit: &[PAST],
    },
    Mutation {
        name: "decode-wide-constant-zero",
        package: Package::Core,
        breaks: "a constant past 64 bits is no integer, not 0",
        file: WORDS_FILE,
        original: "    if field.is_empty() { Some(0) } else { narrowed(field) }",
        replacement: "    Some(narrowed(field).unwrap_or(0))",
        integration: &[],
        unit: &[PAST],
    },
    Mutation {
        name: "decode-wide-bounds-untested",
        package: Package::Core,
        breaks: "a run of a wide column is refused by its chunks' bounds, before a chunk is opened",
        file: "src/engine/columns/scan.rs",
        original: "    Ok(zones.iter().filter(|z| touched(z)).all(fits).then_some(scale))",
        replacement: "    Ok(zones.iter().filter(|z| touched(z)).all(|z| fits(z) || z.run.rows > 0).then_some(scale))",
        integration: &[],
        unit: &[WIDE_RUNS],
    },
    Mutation {
        name: "decode-wide-run-chunks",
        package: Package::Core,
        breaks: "a run of a wide column is judged by the bounds of the chunks it reads alone",
        file: "src/engine/columns/scan.rs",
        original: "    let touched = |z: &Zone| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from;",
        replacement: "    let touched = |z: &Zone| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from.min(0);",
        integration: &[],
        unit: &[WIDE_RUNS],
    },
    Mutation {
        name: "decode-wide-position-chunks",
        package: Package::Core,
        breaks: "positions of a wide column are judged by the bounds of the chunks that hold one of them alone",
        file: "src/engine/columns/scan/positions.rs",
        original: ".is_some_and(|&p| p < z.run.first.saturating_add(z.run.rows))",
        replacement: ".is_some_and(|&p| p >= z.run.first)",
        integration: &[],
        unit: &[WIDE_PLACES],
    },
    Mutation {
        name: "decode-wide-conjunct-unjudged",
        package: Package::Core,
        breaks: "a conjunct on a wide exact column is the scan's to judge",
        file: "src/engine/columns/judged.rs",
        original: "        DataType::Exact { .. }\n        | DataType::Approximate { .. }",
        replacement: "        DataType::Exact { precision, .. } => *precision < 19,\n        DataType::Approximate { .. }",
        integration: &[],
        unit: &["engine::columns::prepared::tests::wide::a_scan_judges_a_wide_column_where_it_reads_integers"],
    },
    Mutation {
        name: "freeze-wide-field-cut",
        package: Package::Core,
        breaks: "a column with a value past 64 bits keeps its fields of 16 bytes, none cut to 8",
        file: FIELDS_FILE,
        original: "            Some(f) => bytes.extend_from_slice(&encoded::narrowed(f)?.to_le_bytes()),",
        replacement: "            Some(f) => bytes.extend_from_slice(f.get(..NARROW)?),",
        integration: &[],
        unit: &["engine::columns::tests::wide::a_column_with_a_value_past_64_bits_keeps_its_16_bytes"],
    },
    Mutation {
        name: "freeze-wide-not-narrowed",
        package: Package::Core,
        breaks: "a wide exact column whose values 64 bits hold is written in fields of 8 bytes",
        file: FIELDS_FILE,
        original: "        if let Some(narrow) = narrowed(t, &records) {",
        replacement: "        if let Some(narrow) = narrowed(t, &records).filter(Records::is_empty) {",
        integration: &[],
        unit: &["engine::columns::tests::wide::a_wide_column_of_values_64_bits_hold_is_written_in_8_bytes"],
    },
];
