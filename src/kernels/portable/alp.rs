//! \[DESIGN\] Decimals made approximate numbers (RULES.md D353, D570): each
//! n of a chunk, of magnitude below 2^51, made an `f64` exactly by adding
//! its bits to those of 2^52 + 2^51 and taking that number away, with no
//! branch, then made its value by its chunk's scale, a loop the compiler
//! vectorizes and wider instructions run four or eight a step.

use crate::kernels::AlpScale;

/// 2^52 + 2^51: of an integer n of magnitude below 2^51, this plus n is
/// held exactly, of one exponent, its ulp 1.
const MAGIC: f64 = 6_755_399_441_055_744.0;
/// The bits of `MAGIC`.
const MAGIC_BITS: u64 = 0x4338_0000_0000_0000;

/// As `Kernels::alp_decode`.
#[inline]
pub fn alp_decode(words: &mut [i64], scale: AlpScale) {
    for w in words {
        let n = f64::from_bits(MAGIC_BITS.wrapping_add_signed(*w)) - MAGIC;
        *w = i64::from_le_bytes(scale.of(n).to_bits().to_le_bytes());
    }
}
