//! \[DESIGN\] Sums and extremes on AVX2 and AVX-512 (RULES.md
//! D570). An exact sum adds each value's low 32 bits and its high 32 bits,
//! both unsigned, in 64-bit lanes, and counts the negatives: the sum of
//! the values is that of their bits unsigned less 2^64 a negative, joined
//! in 128 bits once a chunk, exactly the portable body's sum. Extremes keep
//! a lane's least and greatest, a null's lane left as it was. A
//! selection that picks at least a quarter of the rows it spans runs the
//! loop of all rows over those rows, the rows not picked masked off as
//! nulls are. It takes no vector gather, which measured slower than the
//! portable body's loads a row at a time at every size. A function of a set is called only by an
//! `Isa` of it, made only where the processor has it (`x86::isa`).

use std::arch::x86_64::{
    __m256i, _mm256_add_epi64, _mm256_and_si256, _mm256_blendv_epi8, _mm256_cmpeq_epi64, _mm256_cmpgt_epi64, _mm256_loadu_si256,
    _mm256_set1_epi64x, _mm256_setr_epi64x, _mm256_setzero_si256, _mm256_srli_epi64, _mm256_storeu_si256, _mm256_sub_epi64,
    _mm512_add_epi64, _mm512_and_si512, _mm512_cmplt_epi64_mask, _mm512_loadu_si512, _mm512_mask_add_epi64, _mm512_mask_max_epi64,
    _mm512_mask_min_epi64, _mm512_set1_epi64, _mm512_setzero_si512, _mm512_srli_epi64, _mm512_storeu_si512,
};

use turborel::kernels::portable;
use turborel::storage::Mask;

/// \[DESIGN\] Values summed in lanes before they are joined: below 2^31,
/// so no lane of lows or highs, each below 2^32, passes 64 bits.
const CHUNK: usize = 1 << 30;

/// \[DESIGN\] A selection is run masked if its picks are at least one in
/// this many of the rows it spans; fewer, by the portable body, whose
/// loads are each a picked row's (RULES.md D570).
const DENSE_SHARE: usize = 4;

/// The wide registers of a masked run.
#[derive(Clone, Copy)]
pub(super) enum Wide {
    /// Four lanes, AVX2.
    Avx2,
    /// Eight lanes, AVX-512.
    Avx512,
}

/// The rows `sel` picks, as words of bits from its first row's block of
/// 64 rows, a row past the end of `len` left out, with that block; none
/// if they are fewer than a `DENSE_SHARE`th of the rows they span, span
/// more than a chunk, or do not rise strictly, each picked once.
fn picked_words(sel: &[u32], len: usize) -> Option<(usize, Vec<u64>)> {
    let first = usize::try_from(*sel.first()?).ok()?;
    let last = usize::try_from(*sel.last()?).ok()?;
    let span = last.checked_sub(first)?.saturating_add(1);
    if sel.len().saturating_mul(DENSE_SHARE) < span || span > CHUNK || !sel.is_sorted_by(|a, b| a < b) {
        return None;
    }
    let base = first >> 6_u32;
    let mut words = vec![0_u64; (last >> 6_u32).saturating_sub(base).saturating_add(1)];
    for r in sel.iter().filter_map(|&r| usize::try_from(r).ok()).take_while(|&r| r < len) {
        if let Some(w) = words.get_mut((r >> 6_u32).saturating_sub(base)) {
            *w |= 1_u64 << (r % 64);
        }
    }
    Some((base, words))
}

/// The rows of `values` from the block `base`, `blocks` blocks of them.
fn spanned(values: &[i64], (base, blocks): (usize, usize)) -> &[i64] {
    let start = base.saturating_mul(64).min(values.len());
    let end = base.saturating_add(blocks).saturating_mul(64).min(values.len());
    values.get(start..end).unwrap_or_default()
}

/// `Kernels::sum_i64` of the rows `sel` picks, run masked; none if they
/// are too few for that (`picked_words`).
pub(super) fn sum_picked(wide: Wide, values: &[i64], sel: &[u32]) -> Option<i128> {
    let (base, words) = picked_words(sel, values.len())?;
    let rows = spanned(values, (base, words.len()));
    Some(match wide {
        // SAFETY: a `Wide` is chosen only by an `Isa` of its set, made only
        // where the processor has it (`x86::isa`).
        Wide::Avx2 => unsafe { masked_halves_avx2(rows, &words) },
        // SAFETY: as above.
        Wide::Avx512 => unsafe { masked_halves_avx512(rows, &words) },
    })
}

/// The lows, highs and negatives of the rows of `tail` whose bits from
/// `first` of `picked` are set.
fn tail_picked(tail: &[i64], picked: u64, first: usize) -> (u64, u64, u64) {
    (first..).zip(tail).filter(|&(r, _)| (picked >> r) & 1 == 1).fold((0, 0, 0), |(l, h, n), (_, &v)| {
        let bits = v.cast_unsigned();
        (l.wrapping_add(bits & LOW), h.wrapping_add(bits >> 32_u32), n.wrapping_add(u64::from(v < 0)))
    })
}

/// The sum of the rows of `values` whose bits of `words` are set, a word
/// a block of 64 rows, on AVX2: a row not picked made 0, which adds
/// nothing and is not negative.
#[target_feature(enable = "avx2")]
fn masked_halves_avx2(values: &[i64], words: &[u64]) -> i128 {
    let lane_bits = _mm256_setr_epi64x(1, 2, 4, 8);
    let (zero, low) = (_mm256_setzero_si256(), _mm256_set1_epi64x(LOW.cast_signed()));
    let (mut lows, mut highs, mut negatives) = (zero, zero, zero);
    let mut tails = (0_u64, 0_u64, 0_u64);
    for (rows, &picked) in values.chunks(64).zip(words) {
        let (quads, tail) = rows.as_chunks::<4>();
        for (n, quad) in (0_u32..).zip(quads) {
            // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) };
            let bits = _mm256_set1_epi64x(((picked >> n.wrapping_mul(4)) & 0xF).cast_signed());
            let v = _mm256_and_si256(v, _mm256_cmpeq_epi64(_mm256_and_si256(bits, lane_bits), lane_bits));
            lows = _mm256_add_epi64(lows, _mm256_and_si256(v, low));
            highs = _mm256_add_epi64(highs, _mm256_srli_epi64::<32>(v));
            negatives = _mm256_sub_epi64(negatives, _mm256_cmpgt_epi64(zero, v));
        }
        let (l, h, n) = tail_picked(tail, picked, quads.len().wrapping_mul(4));
        tails = (tails.0.wrapping_add(l), tails.1.wrapping_add(h), tails.2.wrapping_add(n));
    }
    let (l, h, n) = tails;
    joined((lanes_added(lows).wrapping_add(l), lanes_added(highs).wrapping_add(h), lanes_added(negatives).wrapping_add(n)))
}

/// `masked_halves_avx2` on AVX-512: a lane not picked masked off the adds.
#[target_feature(enable = "avx512f,popcnt")]
fn masked_halves_avx512(values: &[i64], words: &[u64]) -> i128 {
    let (zero, low) = (_mm512_setzero_si512(), _mm512_set1_epi64(LOW.cast_signed()));
    let (mut lows, mut highs, mut negatives) = (zero, zero, 0_u64);
    let mut tails = (0_u64, 0_u64, 0_u64);
    for (rows, &picked) in values.chunks(64).zip(words) {
        let (octets, tail) = rows.as_chunks::<8>();
        for (n, octet) in (0_u32..).zip(octets) {
            // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) };
            let kept = (picked >> n.wrapping_mul(8)).to_le_bytes()[0];
            lows = _mm512_mask_add_epi64(lows, kept, lows, _mm512_and_si512(v, low));
            highs = _mm512_mask_add_epi64(highs, kept, highs, _mm512_srli_epi64::<32>(v));
            negatives = negatives.wrapping_add(u64::from((_mm512_cmplt_epi64_mask(v, zero) & kept).count_ones()));
        }
        let (l, h, n) = tail_picked(tail, picked, octets.len().wrapping_mul(8));
        tails = (tails.0.wrapping_add(l), tails.1.wrapping_add(h), tails.2.wrapping_add(n));
    }
    let added = |v| {
        let mut lanes = [0_u64; 8];
        // SAFETY: `lanes` is 64 bytes, and an unaligned store writes exactly them.
        unsafe { _mm512_storeu_si512(lanes.as_mut_ptr().cast(), v) };
        lanes.iter().fold(0_u64, |s, &w| s.wrapping_add(w))
    };
    let (l, h, n) = tails;
    joined((added(lows).wrapping_add(l), added(highs).wrapping_add(h), negatives.wrapping_add(n)))
}

/// `Kernels::min_max_i64` of the rows `sel` picks not null: run masked,
/// or by the portable body if they are too few for that (`picked_words`).
pub(super) fn min_max_picked(wide: Wide, values: &[i64], (nulls, sel): (Option<&Mask>, &[u32])) -> Option<(i64, i64)> {
    let Some((base, words)) = picked_words(sel, values.len()) else {
        return portable::sum::min_max_i64(values, (nulls, Some(sel)));
    };
    let rows = spanned(values, (base, words.len()));
    let held = Held { nulls, picks: Some(&words), base };
    match wide {
        // SAFETY: a `Wide` is chosen only by an `Isa` of its set, made only
        // where the processor has it (`x86::isa`).
        Wide::Avx2 => unsafe { extremes_avx2(rows, held) },
        // SAFETY: as above.
        Wide::Avx512 => unsafe { extremes_avx512(rows, held) },
    }
}

/// The rows an extremes loop holds: not null, and picked if a selection
/// is given, its words from the block `base`, where the rows looped start.
#[derive(Clone, Copy)]
struct Held<'a> {
    /// The nulls of all rows.
    nulls: Option<&'a Mask>,
    /// The bits of the rows picked, a word a block of the rows looped.
    picks: Option<&'a [u64]>,
    /// The block of all rows the rows looped start at.
    base: usize,
}

/// The low half of a word.
const LOW: u64 = 0xFFFF_FFFF;

/// The sum of the bits, unsigned, of the lows, the highs and the count
/// of negatives, joined.
fn joined((lows, highs, negatives): (u64, u64, u64)) -> i128 {
    let unsigned = i128::from(lows).wrapping_add(i128::from(highs) << 32_u32);
    unsigned.wrapping_sub(i128::from(negatives) << 64_u32)
}

/// The lows, highs and negatives of `tail`, a few values, one at a time.
fn tail_halves(tail: &[i64]) -> (u64, u64, u64) {
    tail.iter().fold((0, 0, 0), |(l, h, n), &v| {
        let bits = v.cast_unsigned();
        (l.wrapping_add(bits & LOW), h.wrapping_add(bits >> 32_u32), n.wrapping_add(u64::from(v < 0)))
    })
}

/// The lanes of `v` added.
#[target_feature(enable = "avx2")]
fn lanes_added(v: __m256i) -> u64 {
    let mut words = [0_u64; 4];
    // SAFETY: `words` is 32 bytes, and an unaligned store writes exactly them.
    unsafe { _mm256_storeu_si256(words.as_mut_ptr().cast(), v) };
    words.iter().fold(0, |s, &w| s.wrapping_add(w))
}

/// `Kernels::sum_i64` of all rows, on AVX2.
pub(super) fn sum_avx2(values: &[i64]) -> i128 {
    values
        .chunks(CHUNK)
        .map(|chunk| {
            // SAFETY: an `Isa` of AVX2 is made only where the processor has
            // it (`x86::isa`), and only such a one calls this.
            unsafe { halves_avx2(chunk) }
        })
        .fold(0, i128::wrapping_add)
}

/// The sum of a chunk of `values`, on AVX2.
#[target_feature(enable = "avx2")]
fn halves_avx2(values: &[i64]) -> i128 {
    let (quads, tail) = values.as_chunks::<4>();
    let (zero, low) = (_mm256_setzero_si256(), _mm256_set1_epi64x(LOW.cast_signed()));
    let (mut lows, mut highs, mut negatives) = (zero, zero, zero);
    for quad in quads {
        // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
        let v = unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) };
        lows = _mm256_add_epi64(lows, _mm256_and_si256(v, low));
        highs = _mm256_add_epi64(highs, _mm256_srli_epi64::<32>(v));
        negatives = _mm256_sub_epi64(negatives, _mm256_cmpgt_epi64(zero, v));
    }
    let (l, h, n) = tail_halves(tail);
    joined((lanes_added(lows).wrapping_add(l), lanes_added(highs).wrapping_add(h), lanes_added(negatives).wrapping_add(n)))
}

/// `Kernels::sum_i64` of all rows, on AVX-512.
pub(super) fn sum_avx512(values: &[i64]) -> i128 {
    values
        .chunks(CHUNK)
        .map(|chunk| {
            // SAFETY: an `Isa` of AVX-512 is made only where the processor has
            // it (`x86::isa`), and only such a one calls this.
            unsafe { halves_avx512(chunk) }
        })
        .fold(0, i128::wrapping_add)
}

/// The sum of a chunk of `values`, on AVX-512.
#[target_feature(enable = "avx512f,popcnt")]
fn halves_avx512(values: &[i64]) -> i128 {
    let (octets, tail) = values.as_chunks::<8>();
    let (zero, low) = (_mm512_setzero_si512(), _mm512_set1_epi64(LOW.cast_signed()));
    let (mut lows, mut highs, mut negatives) = (zero, zero, 0_u64);
    for octet in octets {
        // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
        let v = unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) };
        lows = _mm512_add_epi64(lows, _mm512_and_si512(v, low));
        highs = _mm512_add_epi64(highs, _mm512_srli_epi64::<32>(v));
        negatives = negatives.wrapping_add(u64::from(_mm512_cmplt_epi64_mask(v, zero).count_ones()));
    }
    let added = |v| {
        let mut words = [0_u64; 8];
        // SAFETY: `words` is 64 bytes, and an unaligned store writes exactly them.
        unsafe { _mm512_storeu_si512(words.as_mut_ptr().cast(), v) };
        words.iter().fold(0_u64, |s, &w| s.wrapping_add(w))
    };
    let (l, h, n) = tail_halves(tail);
    joined((added(lows).wrapping_add(l), added(highs).wrapping_add(h), negatives.wrapping_add(n)))
}

/// The least and greatest of `values` not null, one at a time, folded
/// into `best`: of a block's tail.
fn folded(best: Option<(i64, i64)>, values: impl Iterator<Item = i64>) -> Option<(i64, i64)> {
    values.fold(best, |best, v| Some(best.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v)))))
}

/// The least of the lanes `least` and the greatest of the lanes `most`,
/// with those of the tails, `tails`: a lane no row held keeps the
/// greatest integer as its least and the least as its greatest, which
/// change neither.
fn reduced((least, most): (&[i64], &[i64]), tails: Option<(i64, i64)>) -> (i64, i64) {
    let lo = least.iter().copied().fold(i64::MAX, i64::min);
    let hi = most.iter().copied().fold(i64::MIN, i64::max);
    tails.map_or((lo, hi), |(l, h)| (lo.min(l), hi.max(h)))
}

/// The bits of the rows of `block` of the rows looped, `len` rows, at
/// most 64, set where `held` holds a row.
fn held_bits(held: Held<'_>, block: usize, len: usize) -> u64 {
    let all = if len >= 64 { u64::MAX } else { (1_u64 << len).wrapping_sub(1) };
    let picked = held.picks.map_or(u64::MAX, |p| p.get(block).copied().unwrap_or(0));
    let null = held.nulls.and_then(|m| m.words().get(held.base.saturating_add(block))).copied().unwrap_or(0);
    all & picked & !null
}

/// `Kernels::min_max_i64` of all rows, on AVX2.
pub(super) fn min_max_avx2(values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { extremes_avx2(values, Held { nulls, picks: None, base: 0 }) }
}

/// The lanes of a register, as words.
#[target_feature(enable = "avx2")]
fn words_of(v: __m256i) -> [i64; 4] {
    let mut words = [0_i64; 4];
    // SAFETY: `words` is 32 bytes, and an unaligned store writes exactly them.
    unsafe { _mm256_storeu_si256(words.as_mut_ptr().cast(), v) };
    words
}

/// `min_max_avx2`'s loop: a lane of a row not held, null or not picked,
/// given the greatest integer for the least and the least for the
/// greatest, so it changes neither.
#[target_feature(enable = "avx2")]
fn extremes_avx2(values: &[i64], held: Held<'_>) -> Option<(i64, i64)> {
    let lane_bits = _mm256_setr_epi64x(1, 2, 4, 8);
    let (mut least, mut most) = (_mm256_set1_epi64x(i64::MAX), _mm256_set1_epi64x(i64::MIN));
    let (mut any, mut tails) = (false, None);
    for (block, rows) in values.chunks(64).enumerate() {
        let held = held_bits(held, block, rows.len());
        any |= held != 0;
        let (quads, tail) = rows.as_chunks::<4>();
        for (n, quad) in (0_u32..).zip(quads) {
            // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) };
            let bits = _mm256_set1_epi64x(((held >> n.wrapping_mul(4)) & 0xF).cast_signed());
            let kept = _mm256_cmpeq_epi64(_mm256_and_si256(bits, lane_bits), lane_bits);
            let low = _mm256_blendv_epi8(_mm256_set1_epi64x(i64::MAX), v, kept);
            let high = _mm256_blendv_epi8(_mm256_set1_epi64x(i64::MIN), v, kept);
            least = _mm256_blendv_epi8(least, low, _mm256_cmpgt_epi64(least, low));
            most = _mm256_blendv_epi8(most, high, _mm256_cmpgt_epi64(high, most));
        }
        let first = quads.len().wrapping_mul(4);
        let kept = (first..).zip(tail).filter(|&(r, _)| (held >> r) & 1 == 1).map(|(_, &v)| v);
        tails = folded(tails, kept);
    }
    any.then(|| reduced((&words_of(least), &words_of(most)), tails))
}

/// `Kernels::min_max_i64` of all rows, on AVX-512.
pub(super) fn min_max_avx512(values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
    // SAFETY: an `Isa` of AVX-512 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { extremes_avx512(values, Held { nulls, picks: None, base: 0 }) }
}

/// `min_max_avx512`'s loop: a lane of a row not held masked off.
#[target_feature(enable = "avx512f")]
fn extremes_avx512(values: &[i64], held: Held<'_>) -> Option<(i64, i64)> {
    let (mut least, mut most) = (_mm512_set1_epi64(i64::MAX), _mm512_set1_epi64(i64::MIN));
    let (mut any, mut tails) = (false, None);
    for (block, rows) in values.chunks(64).enumerate() {
        let held = held_bits(held, block, rows.len());
        any |= held != 0;
        let (octets, tail) = rows.as_chunks::<8>();
        for (n, octet) in (0_u32..).zip(octets) {
            // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) };
            let kept = (held >> n.wrapping_mul(8)).to_le_bytes()[0];
            least = _mm512_mask_min_epi64(least, kept, least, v);
            most = _mm512_mask_max_epi64(most, kept, most, v);
        }
        let first = octets.len().wrapping_mul(8);
        let kept = (first..).zip(tail).filter(|&(r, _)| (held >> r) & 1 == 1).map(|(_, &v)| v);
        tails = folded(tails, kept);
    }
    let mut words = [0_i64; 16];
    let (lows, highs) = words.split_at_mut(8);
    // SAFETY: each half of `words` is 64 bytes, and an unaligned store
    // writes exactly them.
    unsafe { _mm512_storeu_si512(lows.as_mut_ptr().cast(), least) };
    // SAFETY: as above.
    unsafe { _mm512_storeu_si512(highs.as_mut_ptr().cast(), most) };
    any.then(|| reduced((lows, highs), tails))
}
