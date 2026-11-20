//! The mutations of the decode kernels (RULES.md R-TEST-9, D352-D353): a
//! block's value that runs past its word given the wrong rest, ALP's
//! header bit not written, a decimal kept that its factors do not give
//! back, an n made an `f64` off by one ulp, the cache of opened chunks
//! keyed by zone alone, not column too, and a chunk still needed let go.

use super::super::{Mutation, Package};

/// The routine of each width.
const BLOCKS_FILE: &str = "src/engine/columns/packed/blocks.rs";
/// ALP's encoder.
const ALP_FILE: &str = "src/engine/columns/alp.rs";
/// ALP's decoder.
const ALP_DECODE_FILE: &str = "src/engine/columns/alp/decode.rs";
/// The chunks a scan opens once.
const PREPARED_FILE: &str = "src/engine/columns/prepared.rs";
/// The unit test of every width's blocks.
const BLOCKS: &str = "engine::columns::packed::blocks::tests::blocks_read_as_one_at_a_time";
/// The unit test of chunks of decimals read back.
const DECIMALS: &str = "engine::columns::alp::tests::chunks_of_decimals_read_back_bit_for_bit";
/// The unit test of the cache over runs.
const OPENED_ONCE: &str = "engine::columns::prepared::tests::chunks_are_opened_once_for_their_runs";

/// Mechanisms of the decode kernels whose removal must fail their tests.
pub const DECODE: [Mutation; 6] = [
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
        original: "f64::from_bits(MAGIC_BITS.wrapping_add_signed(n)) - MAGIC",
        replacement: "f64::from_bits(MAGIC_BITS.wrapping_add_signed(n).wrapping_add(1)) - MAGIC",
        integration: &[],
        unit: &[DECIMALS],
    },
    Mutation {
        name: "decode-cache-key-zone-only",
        package: Package::Core,
        breaks: "the chunks opened are kept by zone alone, so a column reads another's chunk of the same first row",
        file: PREPARED_FILE,
        original: "let kept = self.columns.entry(column).or_default();",
        replacement: "let kept = self.columns.entry(0).or_default();",
        integration: &[],
        unit: &[OPENED_ONCE, "engine::columns::prepared::tests::positions_read_through_the_cache"],
    },
    Mutation {
        name: "decode-cache-let-go-early",
        package: Package::Core,
        breaks: "a chunk still read by the runs after is let go, so it is opened again for each",
        file: PREPARED_FILE,
        original: "while kept.front().is_some_and(|h| h.end <= first) || kept.len() >= KEPT {",
        replacement: "while kept.front().is_some_and(|h| h.end >= first) || kept.len() >= KEPT {",
        integration: &[],
        unit: &[OPENED_ONCE],
    },
];
