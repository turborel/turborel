//! \[DESIGN\] Sums, extremes and gathers on AVX2 and AVX-512 (RULES.md
//! D570). An exact sum adds each value's low 32 bits and its high 32 bits,
//! both unsigned, in 64-bit lanes, and counts the negatives: the sum of
//! the values is that of their bits unsigned less 2^64 a negative, joined
//! in 128 bits once a chunk, exactly the portable body's sum. Extremes keep
//! a lane's least and greatest, a null's lane left as it was. A gather
//! reads the rows of a selection four (`vpgatherqq`) or eight at a time,
//! each lane's load masked off for a row past the end, which gives 0, as
//! the portable gather gives it. A function of a set is called only by an
//! `Isa` of it, made only where the processor has it (`x86::isa`).

use std::arch::x86_64::{
    __m256i, _mm_cmpgt_epi32, _mm_loadu_si128, _mm_set1_epi32, _mm_xor_si128, _mm256_add_epi64, _mm256_and_si256, _mm256_blendv_epi8,
    _mm256_cmpeq_epi64, _mm256_cmpgt_epi64, _mm256_cmplt_epu32_mask, _mm256_cvtepi32_epi64, _mm256_loadu_si256,
    _mm256_loadu_si256 as load256, _mm256_mask_i32gather_epi64, _mm256_set1_epi32, _mm256_set1_epi64x, _mm256_setr_epi64x,
    _mm256_setzero_si256, _mm256_srli_epi64, _mm256_storeu_si256, _mm256_sub_epi64, _mm512_add_epi64, _mm512_and_si512,
    _mm512_cmplt_epi64_mask, _mm512_loadu_si512, _mm512_mask_i32gather_epi64, _mm512_mask_max_epi64, _mm512_mask_min_epi64,
    _mm512_set1_epi64, _mm512_setzero_si512, _mm512_srli_epi64, _mm512_storeu_si512,
};

use turborel::kernels::portable;
use turborel::storage::Mask;

/// \[DESIGN\] Values summed in lanes before they are joined: below 2^31,
/// so no lane of lows or highs, each below 2^32, passes 64 bits.
const CHUNK: usize = 1 << 30;

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

/// The bits of the rows of a block of `len` rows, at most 64, from `n`
/// of `nulls`, set where a row is held, not null.
fn held_bits(nulls: Option<&Mask>, block: usize, len: usize) -> u64 {
    let all = if len >= 64 { u64::MAX } else { (1_u64 << len).wrapping_sub(1) };
    all & !nulls.and_then(|m| m.words().get(block)).copied().unwrap_or(0)
}

/// `Kernels::min_max_i64` of all rows, on AVX2.
pub(super) fn min_max_avx2(values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { extremes_avx2(values, nulls) }
}

/// The lanes of a register, as words.
#[target_feature(enable = "avx2")]
fn words_of(v: __m256i) -> [i64; 4] {
    let mut words = [0_i64; 4];
    // SAFETY: `words` is 32 bytes, and an unaligned store writes exactly them.
    unsafe { _mm256_storeu_si256(words.as_mut_ptr().cast(), v) };
    words
}

/// `min_max_avx2`'s loop: a lane of a null row given the greatest integer
/// for the least and the least for the greatest, so it changes neither.
#[target_feature(enable = "avx2")]
fn extremes_avx2(values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
    let lane_bits = _mm256_setr_epi64x(1, 2, 4, 8);
    let (mut least, mut most) = (_mm256_set1_epi64x(i64::MAX), _mm256_set1_epi64x(i64::MIN));
    let (mut any, mut tails) = (false, None);
    for (block, rows) in values.chunks(64).enumerate() {
        let held = held_bits(nulls, block, rows.len());
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
    unsafe { extremes_avx512(values, nulls) }
}

/// `min_max_avx512`'s loop: a lane of a null row masked off.
#[target_feature(enable = "avx512f")]
fn extremes_avx512(values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
    let (mut least, mut most) = (_mm512_set1_epi64(i64::MAX), _mm512_set1_epi64(i64::MIN));
    let (mut any, mut tails) = (false, None);
    for (block, rows) in values.chunks(64).enumerate() {
        let held = held_bits(nulls, block, rows.len());
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

/// Whether a gather's 32-bit signed indices reach every value of
/// `values`: if not, the portable gather.
pub(super) fn indexable(values: &[i64]) -> Option<i32> {
    i32::try_from(values.len()).ok()
}

/// `Kernels::gather_i64` on AVX2.
pub(super) fn gather_avx2(values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
    let Some(len) = indexable(values) else { return portable::gather::gather_i64(values, sel, out) };
    let start = out.len();
    out.resize(start.saturating_add(sel.len()), 0);
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { gathered_avx2(values, (sel, len), out.get_mut(start..).unwrap_or_default()) }
}

/// `gather_avx2`'s loop, into `slots`, one a row of `sel`.
#[target_feature(enable = "avx2")]
fn gathered_avx2(values: &[i64], (sel, len): (&[u32], i32), slots: &mut [i64]) {
    let (quads, tail) = sel.as_chunks::<4>();
    let (dests, rest) = slots.as_chunks_mut::<4>();
    // Unsigned order by signed compares: each side's sign bit flipped.
    let (sign, bound) = (_mm_set1_epi32(i32::MIN), _mm_set1_epi32(len ^ i32::MIN));
    for (dest, quad) in dests.iter_mut().zip(quads) {
        // SAFETY: `quad` is 16 bytes, and an unaligned load reads exactly them.
        let rows = unsafe { _mm_loadu_si128(quad.as_ptr().cast()) };
        let within = _mm256_cvtepi32_epi64(_mm_cmpgt_epi32(bound, _mm_xor_si128(rows, sign)));
        let (zero, base) = (_mm256_setzero_si256(), values.as_ptr());
        // SAFETY: a lane is loaded only if its row is below `len`, the
        // length of `values`, below 2^31, so its signed index is the row;
        // every other lane is masked off and reads nothing.
        let got = unsafe { _mm256_mask_i32gather_epi64::<8>(zero, base, rows, within) };
        // SAFETY: `dest` is 32 bytes, and an unaligned store writes exactly them.
        unsafe { _mm256_storeu_si256(dest.as_mut_ptr().cast(), got) };
    }
    for (slot, &r) in rest.iter_mut().zip(tail) {
        *slot = usize::try_from(r).ok().and_then(|r| values.get(r)).copied().unwrap_or(0);
    }
}

/// `Kernels::gather_i64` on AVX-512.
pub(super) fn gather_avx512(values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
    let Some(len) = indexable(values) else { return portable::gather::gather_i64(values, sel, out) };
    let start = out.len();
    out.resize(start.saturating_add(sel.len()), 0);
    // SAFETY: an `Isa` of AVX-512 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { gathered_avx512(values, (sel, len), out.get_mut(start..).unwrap_or_default()) }
}

/// `gather_avx512`'s loop, into `slots`, one a row of `sel`.
#[target_feature(enable = "avx512f,avx512vl")]
fn gathered_avx512(values: &[i64], (sel, len): (&[u32], i32), slots: &mut [i64]) {
    let (octets, tail) = sel.as_chunks::<8>();
    let (dests, rest) = slots.as_chunks_mut::<8>();
    let bound = _mm256_set1_epi32(len);
    for (dest, octet) in dests.iter_mut().zip(octets) {
        // SAFETY: `octet` is 32 bytes, and an unaligned load reads exactly them.
        let rows = unsafe { load256(octet.as_ptr().cast()) };
        let within = _mm256_cmplt_epu32_mask(rows, bound);
        let (zero, base) = (_mm512_setzero_si512(), values.as_ptr());
        // SAFETY: as `gathered_avx2`'s: a lane is loaded only if its row is
        // below the length of `values`, below 2^31.
        let got = unsafe { _mm512_mask_i32gather_epi64::<8>(zero, within, rows, base) };
        // SAFETY: `dest` is 64 bytes, and an unaligned store writes exactly them.
        unsafe { _mm512_storeu_si512(dest.as_mut_ptr().cast(), got) };
    }
    for (slot, &r) in rest.iter_mut().zip(tail) {
        *slot = usize::try_from(r).ok().and_then(|r| values.get(r)).copied().unwrap_or(0);
    }
}

/// The values of `values` at `sel`, below `len` its length, into `slots`,
/// one a row, on AVX2: 0 of a row past the end.
pub(super) fn picked_avx2(values: &[i64], (sel, len): (&[u32], i32), slots: &mut [i64]) {
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { gathered_avx2(values, (sel, len), slots) }
}

/// `picked_avx2`, on AVX-512.
pub(super) fn picked_avx512(values: &[i64], (sel, len): (&[u32], i32), slots: &mut [i64]) {
    // SAFETY: an `Isa` of AVX-512 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { gathered_avx512(values, (sel, len), slots) }
}
