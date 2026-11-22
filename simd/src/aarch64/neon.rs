//! \[DESIGN\] Selections, sums, extremes and decimals on NEON (RULES.md
//! D570). A selection compares two rows a register, narrows four
//! registers' truths to a byte of eight rows (`vmovn`, then the lanes'
//! bits added, `vaddv`), and stores each byte's places widened from
//! `rows::offsets`, eight a byte. A sum adds each value's low and high 32
//! bits, both unsigned, in 64-bit lanes and counts the negatives, the
//! portable sum exactly; extremes keep a lane's least and greatest, a
//! null's lane left as it was; decimals are made as the portable body
//! makes them, two a step, no fused multiply. A function here is called
//! only by an `Isa` of NEON, made only where the processor has it
//! (`aarch64::isa`).

use std::arch::aarch64::{
    float64x2_t, int64x2_t, uint8x8_t, uint64x2_t, vaddq_s64, vaddq_u32, vaddq_u64, vaddv_u8, vaddvq_u64, vandq_u64, vbslq_s64, vceqq_f64,
    vceqq_s64, vcgeq_f64, vcgeq_s64, vcgtq_f64, vcgtq_s64, vcleq_f64, vcleq_s64, vcltq_f64, vcltq_s64, vcltzq_s64, vcombine_u16,
    vcombine_u32, vdivq_f64, vdupq_n_f64, vdupq_n_s64, vdupq_n_u32, vdupq_n_u64, vget_low_u16, vgetq_lane_s64, vld1_u8, vld1q_f64,
    vld1q_s64, vld1q_u64, vmovl_high_u16, vmovl_u8, vmovl_u16, vmovn_u16, vmovn_u32, vmovn_u64, vmulq_f64, vorrq_u64,
    vreinterpretq_f64_s64, vreinterpretq_s64_f64, vreinterpretq_u64_s64, vshrq_n_u64, vst1q_s64, vst1q_u32, vsubq_f64, vsubq_u64,
};

use turborel::kernels::{AlpScale, portable};
use turborel::storage::Mask;
use turborel::value::CompOp;

use crate::rows::{self, WORD_BITS, chunk_of, holds_f64, holds_i64, tail_bits, too_long};

/// The lanes' bits of a byte, as eight rows' truths are added into one.
const BITS: [u8; 8] = [1, 2, 4, 8, 16, 32, 64, 128];

/// The byte of eight rows' truths, two a register, the first row lowest.
#[target_feature(enable = "neon")]
fn byte_of(m: [uint64x2_t; 4]) -> u64 {
    let [first, second, third, fourth] = m;
    let low = vcombine_u32(vmovn_u64(first), vmovn_u64(second));
    let high = vcombine_u32(vmovn_u64(third), vmovn_u64(fourth));
    let narrow: uint8x8_t = vmovn_u16(vcombine_u16(vmovn_u32(low), vmovn_u32(high)));
    // SAFETY: `BITS` is 8 bytes, and the load reads exactly them.
    let bits = unsafe { vld1_u8(BITS.as_ptr()) };
    u64::from(vaddv_u8(std::arch::aarch64::vand_u8(narrow, bits)))
}

/// Of `v op c` of two integers, the truths, `op` matched a call.
#[target_feature(enable = "neon")]
fn compare_i64(op: CompOp, v: int64x2_t, c: int64x2_t) -> uint64x2_t {
    match op {
        CompOp::Eq | CompOp::Ne => vceqq_s64(v, c),
        CompOp::Lt => vcltq_s64(v, c),
        CompOp::Gt => vcgtq_s64(v, c),
        CompOp::Le => vcleq_s64(v, c),
        CompOp::Ge => vcgeq_s64(v, c),
    }
}

/// Of `v op c` of two approximate numbers, the truths: ordered, so a NaN
/// holds of none, `<>` as `<` or `>`.
#[target_feature(enable = "neon")]
fn compare_f64(op: CompOp, v: float64x2_t, c: float64x2_t) -> uint64x2_t {
    match op {
        CompOp::Eq => vceqq_f64(v, c),
        CompOp::Ne => vorrq_u64(vcltq_f64(v, c), vcgtq_f64(v, c)),
        CompOp::Lt => vcltq_f64(v, c),
        CompOp::Gt => vcgtq_f64(v, c),
        CompOp::Le => vcleq_f64(v, c),
        CompOp::Ge => vcgeq_f64(v, c),
    }
}

/// The truth bits of `values`, at most a chunk, into `words`.
#[target_feature(enable = "neon")]
fn truth_i64(values: &[i64], (op, c): (CompOp, i64), words: &mut [u64]) {
    let constant = vdupq_n_s64(c);
    let flip = if matches!(op, CompOp::Ne) { 0xFF } else { 0 };
    for (word, block) in words.iter_mut().zip(values.chunks(WORD_BITS)) {
        let (octets, tail) = block.as_chunks::<8>();
        let mut w = 0_u64;
        for (n, octet) in (0_u32..).zip(octets) {
            let (pairs, _) = octet.as_chunks::<2>();
            let m: [uint64x2_t; 4] = std::array::from_fn(|k| {
                let pair = pairs.get(k).copied().unwrap_or_default();
                // SAFETY: `pair` is 16 bytes, and the load reads exactly them.
                compare_i64(op, unsafe { vld1q_s64(pair.as_ptr()) }, constant)
            });
            w |= (byte_of(m) ^ flip).wrapping_shl(n.wrapping_mul(8));
        }
        let at = u32::try_from(octets.len()).unwrap_or(0).wrapping_mul(8);
        *word = w | tail_bits(tail, at, |v| holds_i64(op, v, c));
    }
}

/// The truth bits of `values`, at most a chunk, into `words`.
#[target_feature(enable = "neon")]
fn truth_f64(values: &[f64], (op, c): (CompOp, f64), words: &mut [u64]) {
    let constant = vdupq_n_f64(c);
    for (word, block) in words.iter_mut().zip(values.chunks(WORD_BITS)) {
        let (octets, tail) = block.as_chunks::<8>();
        let mut w = 0_u64;
        for (n, octet) in (0_u32..).zip(octets) {
            let (pairs, _) = octet.as_chunks::<2>();
            let m: [uint64x2_t; 4] = std::array::from_fn(|k| {
                let pair = pairs.get(k).copied().unwrap_or_default();
                // SAFETY: `pair` is 16 bytes, and the load reads exactly them.
                compare_f64(op, unsafe { vld1q_f64(pair.as_ptr()) }, constant)
            });
            w |= byte_of(m).wrapping_shl(n.wrapping_mul(8));
        }
        let at = u32::try_from(octets.len()).unwrap_or(0).wrapping_mul(8);
        *word = w | tail_bits(tail, at, |v| holds_f64(op, v, c));
    }
}

/// The places of the bits set of `words`, from row `base`, into `slots`,
/// eight a byte: none written past `slots`.
#[target_feature(enable = "neon")]
fn emit(words: &[u64], base: u32, slots: &mut [u32]) -> usize {
    if slots.len() < words.len().saturating_mul(WORD_BITS).saturating_add(8) {
        return rows::emit_scalar(words, base, slots);
    }
    let table = rows::offsets();
    let mut k = 0_usize;
    for (n, &w) in (0_u32..).zip(words) {
        if w == 0 {
            continue;
        }
        let row = base.wrapping_add(n.wrapping_mul(64));
        for (j, byte) in (0_u32..).zip(w.to_le_bytes()) {
            let (Some(places), Some(slot)) = (table.get(usize::from(byte)), slots.get_mut(k..).and_then(<[u32]>::first_chunk_mut::<8>))
            else {
                break;
            };
            // SAFETY: `places` is 8 bytes, and the load reads exactly them.
            let wide = vmovl_u8(unsafe { vld1_u8(places.as_ptr()) });
            let at = vdupq_n_u32(row.wrapping_add(j.wrapping_mul(8)));
            let (low, high) = (vaddq_u32(vmovl_u16(vget_low_u16(wide)), at), vaddq_u32(vmovl_high_u16(wide), at));
            let (first, second) = slot.split_at_mut(4);
            // SAFETY: `first` is 4 places, 16 bytes, and the store writes exactly them.
            unsafe { vst1q_u32(first.as_mut_ptr(), low) };
            // SAFETY: as above, of `second`.
            unsafe { vst1q_u32(second.as_mut_ptr(), high) };
            k = k.wrapping_add(usize::try_from(byte.count_ones()).unwrap_or(0));
        }
    }
    k
}

/// `emit`, as a set's stores are given to `rows`.
fn stores(words: &[u64], base: u32, slots: &mut [u32]) -> usize {
    // SAFETY: called only by an `Isa` of NEON, made only where the
    // processor has it (`aarch64::isa`); the stores write within `slots`.
    unsafe { emit(words, base, slots) }
}

/// `Kernels::select_i64` of all rows.
pub(super) fn select_i64(values: &[i64], pred: (CompOp, i64), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    if too_long(values.len()) {
        return portable::select::select_i64(values, pred, (nulls, None), out);
    }
    let truth = |from: usize, words: &mut [u64]| {
        // SAFETY: as `stores`.
        unsafe { truth_i64(chunk_of(values, from, words), pred, words) }
    };
    rows::select_rows(values.len(), nulls, out, (truth, stores));
}

/// `Kernels::select_f64` of all rows.
pub(super) fn select_f64(values: &[f64], pred: (CompOp, f64), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    if too_long(values.len()) {
        return portable::select::select_f64(values, pred, (nulls, None), out);
    }
    let truth = |from: usize, words: &mut [u64]| {
        // SAFETY: as `stores`.
        unsafe { truth_f64(chunk_of(values, from, words), pred, words) }
    };
    rows::select_rows(values.len(), nulls, out, (truth, stores));
}

/// `Kernels::select_mask` of all rows.
pub(super) fn select_mask(bits: &[u64], nulls: Option<&Mask>, out: &mut Vec<u32>) {
    rows::select_mask(bits, nulls, out, stores);
}

/// `Kernels::select_codes` of all rows.
pub(super) fn select_codes(given: (&[u32], &[u64]), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    rows::select_codes(given, nulls, out, stores);
}

/// `Kernels::sum_i64` of all rows.
pub(super) fn sum_i64(values: &[i64]) -> i128 {
    values
        .chunks(1 << 30)
        .map(|chunk| {
            // SAFETY: as `stores`.
            unsafe { halves(chunk) }
        })
        .fold(0, i128::wrapping_add)
}

/// The sum of a chunk of `values`, of fewer than 2^31: the lows, highs and
/// negatives in lanes, joined.
#[target_feature(enable = "neon")]
fn halves(values: &[i64]) -> i128 {
    let (pairs, tail) = values.as_chunks::<2>();
    let low = vdupq_n_u64(0xFFFF_FFFF);
    let (mut lows, mut highs, mut negatives) = (vdupq_n_u64(0), vdupq_n_u64(0), vdupq_n_u64(0));
    for pair in pairs {
        // SAFETY: `pair` is 16 bytes, and the load reads exactly them.
        let v = unsafe { vld1q_s64(pair.as_ptr()) };
        let bits = vreinterpretq_u64_s64(v);
        lows = vaddq_u64(lows, vandq_u64(bits, low));
        highs = vaddq_u64(highs, vshrq_n_u64::<32>(bits));
        negatives = vsubq_u64(negatives, vcltzq_s64(v));
    }
    let (mut l, mut h, mut n) = (vaddvq_u64(lows), vaddvq_u64(highs), vaddvq_u64(negatives));
    for &v in tail {
        let bits = v.cast_unsigned();
        (l, h, n) = (l.wrapping_add(bits & 0xFFFF_FFFF), h.wrapping_add(bits >> 32_u32), n.wrapping_add(u64::from(v < 0)));
    }
    let unsigned = i128::from(l).wrapping_add(i128::from(h) << 32_u32);
    unsigned.wrapping_sub(i128::from(n) << 64_u32)
}

/// `Kernels::min_max_i64` of all rows.
pub(super) fn min_max_i64(values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
    // SAFETY: as `stores`.
    unsafe { extremes(values, nulls) }
}

/// The least and greatest of `values` not null, one at a time, into `best`.
fn folded(best: Option<(i64, i64)>, values: impl Iterator<Item = i64>) -> Option<(i64, i64)> {
    values.fold(best, |best, v| Some(best.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v)))))
}

/// `min_max_i64`'s loop: a null's lane given the greatest integer for the
/// least and the least for the greatest, so it changes neither.
#[target_feature(enable = "neon")]
fn extremes(values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
    let (mut least, mut most) = (vdupq_n_s64(i64::MAX), vdupq_n_s64(i64::MIN));
    let (mut any, mut tails) = (false, None);
    let lane_bits = [1_u64, 2];
    // SAFETY: `lane_bits` is 16 bytes, and the load reads exactly them.
    let lanes = unsafe { vld1q_u64(lane_bits.as_ptr()) };
    for (block, rows) in values.chunks(64).enumerate() {
        let all = if rows.len() >= 64 { u64::MAX } else { (1_u64 << rows.len()).wrapping_sub(1) };
        let held = all & !nulls.and_then(|m| m.words().get(block)).copied().unwrap_or(0);
        any |= held != 0;
        let (pairs, tail) = rows.as_chunks::<2>();
        for (n, pair) in (0_u32..).zip(pairs) {
            // SAFETY: `pair` is 16 bytes, and the load reads exactly them.
            let v = unsafe { vld1q_s64(pair.as_ptr()) };
            let bits = vdupq_n_u64((held >> n.wrapping_mul(2)) & 3);
            let kept = vceqq_s64(
                std::arch::aarch64::vreinterpretq_s64_u64(vandq_u64(bits, lanes)),
                std::arch::aarch64::vreinterpretq_s64_u64(lanes),
            );
            let low = vbslq_s64(kept, v, vdupq_n_s64(i64::MAX));
            let high = vbslq_s64(kept, v, vdupq_n_s64(i64::MIN));
            least = vbslq_s64(vcltq_s64(low, least), low, least);
            most = vbslq_s64(vcgtq_s64(high, most), high, most);
        }
        let first = pairs.len().wrapping_mul(2);
        tails = folded(tails, (first..).zip(tail).filter(|&(r, _)| (held >> r) & 1 == 1).map(|(_, &v)| v));
    }
    let (lows, highs) = ([vgetq_lane_s64::<0>(least), vgetq_lane_s64::<1>(least)], [vgetq_lane_s64::<0>(most), vgetq_lane_s64::<1>(most)]);
    any.then(|| {
        let lo = lows.into_iter().fold(i64::MAX, i64::min);
        let hi = highs.into_iter().fold(i64::MIN, i64::max);
        tails.map_or((lo, hi), |(l, h)| (lo.min(l), hi.max(h)))
    })
}

/// `Kernels::alp_decode`.
pub(super) fn alp_decode(words: &mut [i64], scale: AlpScale) {
    // SAFETY: as `stores`.
    unsafe { decoded(words, scale) }
}

/// `alp_decode`'s loop, two values a step.
#[target_feature(enable = "neon")]
fn decoded(words: &mut [i64], scale: AlpScale) {
    let (pairs, tail) = words.as_chunks_mut::<2>();
    let (bits, magic) = (vdupq_n_s64(0x4338_0000_0000_0000), vdupq_n_f64(6_755_399_441_055_744.0));
    for pair in pairs.iter_mut() {
        // SAFETY: `pair` is 16 bytes, and the load reads exactly them.
        let v = unsafe { vld1q_s64(pair.as_ptr()) };
        let n = vsubq_f64(vreinterpretq_f64_s64(vaddq_s64(v, bits)), magic);
        let value = match scale {
            AlpScale::Factored(power, fraction) => vmulq_f64(vmulq_f64(n, vdupq_n_f64(power)), vdupq_n_f64(fraction)),
            AlpScale::Divided(power) => vdivq_f64(n, vdupq_n_f64(power)),
        };
        let stored = vreinterpretq_s64_f64(value);
        // SAFETY: `pair` is 16 bytes, and the store writes exactly them.
        unsafe { vst1q_s64(pair.as_mut_ptr(), stored) };
    }
    portable::alp::alp_decode(tail, scale);
}
