//! \[DESIGN\] What every set's selections share (RULES.md D570): a vector's
//! rows judged a chunk of 2,048 at a time into words of truth bits, 64
//! rows a word, by the set's compares; the null words taken out of them;
//! then the places of the bits set written by the set's stores, eight or
//! sixteen places a store from a table of each byte's bits
//! (`offsets`), the count of places moved on by the byte's bits set, as
//! Polychroniou, Raghavan and Ross (SIGMOD 2015) §4 write a selection's
//! places with no branch a row.

use turborel::storage::Mask;
use turborel::value::CompOp;

/// Words of truth bits a chunk is judged into: 2,048 rows.
pub(crate) const CHUNK_WORDS: usize = 32;

/// Bits in a word.
pub(crate) const WORD_BITS: usize = 64;

/// The shift from a row's place to its word's.
pub(crate) const WORD_SHIFT: u32 = 6;

/// Rows in a chunk.
pub(crate) const CHUNK_ROWS: usize = CHUNK_WORDS * WORD_BITS;

/// Places a set's store may write past the last place it keeps: sixteen,
/// AVX-512's, the widest.
pub(crate) const SLACK: usize = 16;

/// \[DESIGN\] Of each byte, the places of its bits set, lowest first, the
/// rest 0: the eight places a byte's rows give, stored whole. Made once.
pub(crate) fn offsets() -> &'static [[u8; 8]; 256] {
    static TABLE: std::sync::OnceLock<[[u8; 8]; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [[0_u8; 8]; 256];
        for (byte, places) in (0_u32..).zip(table.iter_mut()) {
            let set = (0_u8..8).filter(|&bit| (byte >> bit) & 1 == 1);
            places.iter_mut().zip(set).for_each(|(slot, bit)| *slot = bit);
        }
        table
    })
}

/// The word of `nulls` at `w`, none past its words.
fn null_word(nulls: Option<&Mask>, w: usize) -> u64 {
    nulls.and_then(|m| m.words().get(w)).copied().unwrap_or(0)
}

/// Appends to `out` the rows below `len`, not null, whose truth bit
/// `truth` sets: each chunk's words by `truth(from, words)`, its rows from
/// `from` with no bit past `len` set; their places by `emit(words, from,
/// slots)`, which writes them to the start of `slots`, given room for all
/// the chunk's rows and `SLACK` more, and returns how many it wrote.
pub(crate) fn select_rows(
    len: usize,
    nulls: Option<&Mask>,
    out: &mut Vec<u32>,
    (mut truth, mut emit): (impl FnMut(usize, &mut [u64]), impl FnMut(&[u64], u32, &mut [u32]) -> usize),
) {
    let start = out.len();
    out.resize(start.saturating_add(len).saturating_add(SLACK), 0);
    let mut k = start;
    let mut words = [0_u64; CHUNK_WORDS];
    for from in (0..len).step_by(CHUNK_ROWS) {
        let rows = len.saturating_sub(from).min(CHUNK_ROWS);
        let these = words.get_mut(..rows.div_ceil(WORD_BITS)).unwrap_or_default();
        these.fill(0);
        truth(from, these);
        let first = from >> WORD_SHIFT;
        for (n, w) in these.iter_mut().enumerate() {
            *w &= !null_word(nulls, first.saturating_add(n));
        }
        let base = u32::try_from(from).unwrap_or(u32::MAX);
        k = k.saturating_add(emit(these, base, out.get_mut(k..).unwrap_or_default()));
    }
    out.truncate(k);
}

/// The places of the bits set of `words`, from row `base`, written to
/// `slots` one at a time with no branch a row, as the portable loop
/// writes them: where a set's store has no room left.
pub(crate) fn emit_scalar(words: &[u64], base: u32, slots: &mut [u32]) -> usize {
    let mut k = 0_usize;
    for (n, &w) in (0_u32..).zip(words) {
        let mut rest = w;
        while rest != 0 {
            let bit = rest.trailing_zeros();
            if let Some(slot) = slots.get_mut(k) {
                *slot = base.wrapping_add(n.wrapping_mul(64)).wrapping_add(bit);
                k = k.saturating_add(1);
            }
            rest &= rest.wrapping_sub(1);
        }
    }
    k
}

/// Whether `v op c` holds, as the portable loop judges an integer.
pub(crate) fn holds_i64(op: CompOp, v: i64, c: i64) -> bool {
    match op {
        CompOp::Eq => v == c,
        CompOp::Ne => v != c,
        CompOp::Lt => v < c,
        CompOp::Gt => v > c,
        CompOp::Le => v <= c,
        CompOp::Ge => v >= c,
    }
}

/// Whether `v op c` holds, as the portable loop judges an approximate
/// number: no NaN is equal, unequal, less or greater than any number.
pub(crate) fn holds_f64(op: CompOp, v: f64, c: f64) -> bool {
    match op {
        CompOp::Eq => v.partial_cmp(&c).is_some_and(std::cmp::Ordering::is_eq),
        CompOp::Ne => v.partial_cmp(&c).is_some_and(std::cmp::Ordering::is_ne),
        CompOp::Lt => v < c,
        CompOp::Gt => v > c,
        CompOp::Le => v <= c,
        CompOp::Ge => v >= c,
    }
}

/// The truth bits of the rows of `tail`, the last of a word's, from bit
/// `at`.
pub(crate) fn tail_bits<T: Copy>(tail: &[T], at: u32, holds: impl Fn(T) -> bool) -> u64 {
    (at..).zip(tail).fold(0, |w, (bit, &v)| w | u64::from(holds(v)).wrapping_shl(bit))
}

/// Of the rows of `values` from `from`, at most a chunk, their truth bits
/// into `words`.
pub(crate) fn chunk_of<'a, T>(values: &'a [T], from: usize, words: &[u64]) -> &'a [T] {
    let rows = words.len().saturating_mul(WORD_BITS);
    values.get(from..).unwrap_or_default().get(..rows).unwrap_or_else(|| values.get(from..).unwrap_or_default())
}

/// Whether `len` rows are too many for a place of 32 bits.
pub(crate) fn too_long(len: usize) -> bool {
    u32::try_from(len).is_err()
}

/// `Kernels::select_mask` of all rows: the bits as they are, none past the
/// rows, their places by `emit`, a set's stores.
pub(crate) fn select_mask(bits: &[u64], nulls: Option<&Mask>, out: &mut Vec<u32>, emit: impl FnMut(&[u64], u32, &mut [u32]) -> usize) {
    let count = nulls.map_or_else(|| bits.len().saturating_mul(WORD_BITS), Mask::len);
    if too_long(count) {
        return turborel::kernels::portable::select::select_mask(bits, (nulls, None), out);
    }
    let truth = |from: usize, words: &mut [u64]| {
        let first = from >> WORD_SHIFT;
        for (n, w) in words.iter_mut().enumerate() {
            *w = bits.get(first.saturating_add(n)).copied().unwrap_or(0);
        }
        let rows = count.saturating_sub(from).min(words.len().saturating_mul(WORD_BITS));
        if let Some(last) = words.last_mut()
            && !rows.is_multiple_of(WORD_BITS)
        {
            *last &= (1_u64 << (rows & (WORD_BITS - 1))).wrapping_sub(1);
        }
    };
    select_rows(count, nulls, out, (truth, emit));
}

/// `Kernels::select_codes` of all rows: each code's bit of `keep` read
/// into its word, their places by `emit`, a set's stores: on NEON, as x86
/// runs the portable body for it.
#[cfg(target_arch = "aarch64")]
pub(crate) fn select_codes(
    (codes, keep): (&[u32], &[u64]),
    nulls: Option<&Mask>,
    out: &mut Vec<u32>,
    emit: impl FnMut(&[u64], u32, &mut [u32]) -> usize,
) {
    if too_long(codes.len()) {
        return turborel::kernels::portable::select::select_codes(codes, keep, (nulls, None), out);
    }
    let kept = |code: u32| {
        let n = usize::try_from(code).unwrap_or(usize::MAX);
        keep.get(n >> WORD_SHIFT).map_or(0, |w| (w >> (n & (WORD_BITS - 1))) & 1)
    };
    let truth = |from: usize, words: &mut [u64]| {
        let chunk = chunk_of(codes, from, words);
        for (w, block) in words.iter_mut().zip(chunk.chunks(WORD_BITS)) {
            *w = (0_u32..).zip(block).fold(0, |bits, (bit, &code)| bits | kept(code).wrapping_shl(bit));
        }
    };
    select_rows(codes.len(), nulls, out, (truth, emit));
}
