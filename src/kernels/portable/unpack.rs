//! \[DESIGN\] The portable body of unpacking (RULES.md D352, D354): the
//! column store's own block unpacker, a routine for each width that the
//! compiler unrolls, so a set of kernels that unpacks on wider
//! instructions is held to what the chunks themselves decode.

/// As `Kernels::unpack`.
#[inline]
pub fn unpack(bytes: &[u8], (bits, least): (u32, i64), (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
    crate::engine::columns::unpack_words(bytes, (bits, least), (from, to), out)
}
