//! \[DESIGN\] Distance sums on NEON (RULES.md D571) keep the portable
//! bodies' sixteen lanes in four registers of four, over the same whole
//! chunks, each lane summed alone by a multiply or subtract and then an
//! add, never fused, so every lane rounds as the portable lane does. INT8
//! widens to `i32` lanes. The lanes are stored and the portable `finish_*` sums them
//! and the tail, so the result is the portable one bit for bit. FLOAT16
//! has no conversion here a stable compiler gives, so it runs its portable
//! body. A function here is called only by an `Isa` of NEON, made only
//! where the processor has it (`aarch64::isa`).

use std::arch::aarch64::{
    float32x4_t, int32x4_t, vabsq_f32, vabsq_s32, vaddq_f32, vaddq_s32, vdupq_n_f32, vdupq_n_s32, vget_low_s8, vget_low_s16, vld1q_s8,
    vld1q_u8, vmovl_high_s8, vmovl_high_s16, vmovl_s8, vmovl_s16, vmulq_f32, vmulq_s32, vreinterpretq_f32_u8, vst1q_f32, vst1q_s32,
    vsubq_f32, vsubq_s32,
};

use turborel::kernels::DistanceTerm;
use turborel::kernels::portable::distance::{ByteLanes, LANES, Lanes, SINGLE_STEP};

/// `DistanceTerm::Dot` as a body's constant.
const DOT: u8 = 0;
/// `DistanceTerm::Square` as a body's constant.
const SQUARE: u8 = 1;
/// `DistanceTerm::Gap` as a body's constant.
const GAP: u8 = 2;

/// The `f32` offsets of a chunk's four registers.
const QUARTERS: [usize; 4] = [0, 4, 8, 12];
/// The byte offsets of a FLOAT32 chunk's four registers.
const QUARTER_BYTES: [usize; 4] = [0, 16, 32, 48];
/// The bytes of a chunk of INT8 elements.
const BYTE_STEP: usize = LANES;

/// Two vectors' starts, each of whole chunks of sixteen elements.
pub(super) type Starts = (*const u8, *const u8);

/// The term of four pairs.
#[target_feature(enable = "neon")]
fn term4<const TERM: u8>(p: float32x4_t, q: float32x4_t) -> float32x4_t {
    if TERM == DOT {
        vmulq_f32(p, q)
    } else if TERM == SQUARE {
        let d = vsubq_f32(p, q);
        vmulq_f32(d, d)
    } else {
        vabsq_f32(vsubq_f32(p, q))
    }
}

/// The four `f32`s of the 16 bytes at `p`, little-endian, the processor's
/// order.
///
/// # Safety
///
/// The processor has NEON, and `p` points to 16 readable bytes.
#[target_feature(enable = "neon")]
unsafe fn load4(p: *const u8) -> float32x4_t {
    // SAFETY: the caller vouches for 16 bytes, and the load reads exactly
    // them, at any alignment.
    vreinterpretq_f32_u8(unsafe { vld1q_u8(p) })
}

/// Four registers of four lanes stored in order.
#[target_feature(enable = "neon")]
fn stored4(acc: [float32x4_t; 4]) -> Lanes {
    let mut out = [0.0_f32; LANES];
    let at = out.as_mut_ptr();
    for (v, quarter) in acc.into_iter().zip(QUARTERS) {
        // SAFETY: `out` holds sixteen `f32`s; each store writes four of them.
        unsafe { vst1q_f32(at.wrapping_add(quarter), v) };
    }
    out
}

/// The lanes of `TERM` summed over `chunks` chunks of sixteen `f32`s.
///
/// # Safety
///
/// The processor has NEON, and each start points to `chunks` readable
/// chunks of sixteen `f32`s, at any alignment.
#[target_feature(enable = "neon")]
unsafe fn lanes_neon<const TERM: u8>((mut x, mut y): Starts, chunks: usize) -> Lanes {
    let mut acc = [vdupq_n_f32(0.0); 4];
    for _ in 0..chunks {
        for (sum, quarter) in acc.iter_mut().zip(QUARTER_BYTES) {
            // SAFETY: a quarter of a chunk the caller vouches for.
            let p = unsafe { load4(x.wrapping_add(quarter)) };
            // SAFETY: as above, of the other vector.
            let q = unsafe { load4(y.wrapping_add(quarter)) };
            *sum = vaddq_f32(*sum, term4::<TERM>(p, q));
        }
        (x, y) = (x.wrapping_add(SINGLE_STEP), y.wrapping_add(SINGLE_STEP));
    }
    stored4(acc)
}

/// The lanes of `term` on NEON, of FLOAT32 elements.
///
/// # Safety
///
/// As `lanes_neon`.
pub(super) unsafe fn floats_neon(term: DistanceTerm, starts: Starts, chunks: usize) -> Lanes {
    let body: unsafe fn(Starts, usize) -> Lanes = match term {
        DistanceTerm::Dot => lanes_neon::<DOT>,
        DistanceTerm::Square => lanes_neon::<SQUARE>,
        DistanceTerm::Gap => lanes_neon::<GAP>,
    };
    // SAFETY: the caller vouches for the processor and the chunks.
    unsafe { body(starts, chunks) }
}

/// The lanes of `a·a`, `b·b` and `a·b` over `chunks` chunks of `f32`s.
///
/// # Safety
///
/// As `lanes_neon`.
#[target_feature(enable = "neon")]
pub(super) unsafe fn cosine_neon((mut x, mut y): Starts, chunks: usize) -> (Lanes, Lanes, Lanes) {
    let zero = [vdupq_n_f32(0.0); 4];
    let (mut aa, mut bb, mut ab) = (zero, zero, zero);
    for _ in 0..chunks {
        for (((sa, sb), sab), quarter) in aa.iter_mut().zip(bb.iter_mut()).zip(ab.iter_mut()).zip(QUARTER_BYTES) {
            // SAFETY: a quarter of a chunk the caller vouches for.
            let p = unsafe { load4(x.wrapping_add(quarter)) };
            // SAFETY: as above, of the other vector.
            let q = unsafe { load4(y.wrapping_add(quarter)) };
            *sa = vaddq_f32(*sa, vmulq_f32(p, p));
            *sb = vaddq_f32(*sb, vmulq_f32(q, q));
            *sab = vaddq_f32(*sab, vmulq_f32(p, q));
        }
        (x, y) = (x.wrapping_add(SINGLE_STEP), y.wrapping_add(SINGLE_STEP));
    }
    (stored4(aa), stored4(bb), stored4(ab))
}

/// The term of four pairs of INT8 elements widened to `i32`, wrapping.
#[target_feature(enable = "neon")]
fn byte_term4<const TERM: u8>(p: int32x4_t, q: int32x4_t) -> int32x4_t {
    if TERM == DOT {
        vmulq_s32(p, q)
    } else if TERM == SQUARE {
        let d = vsubq_s32(p, q);
        vmulq_s32(d, d)
    } else {
        vabsq_s32(vsubq_s32(p, q))
    }
}

/// The sixteen INT8s at `p`, widened to four registers of `i32`s.
///
/// # Safety
///
/// The processor has NEON, and `p` points to 16 readable bytes.
#[target_feature(enable = "neon")]
unsafe fn widened(p: *const u8) -> [int32x4_t; 4] {
    // SAFETY: the caller vouches for 16 bytes, and the load reads exactly
    // them.
    let v = unsafe { vld1q_s8(p.cast()) };
    let (low, high) = (vmovl_s8(vget_low_s8(v)), vmovl_high_s8(v));
    [vmovl_s16(vget_low_s16(low)), vmovl_high_s16(low), vmovl_s16(vget_low_s16(high)), vmovl_high_s16(high)]
}

/// The `i32` lanes of `TERM` over `chunks` chunks of sixteen INT8s.
///
/// # Safety
///
/// The processor has NEON, and each start points to `chunks` readable
/// chunks of sixteen bytes.
#[target_feature(enable = "neon")]
unsafe fn bytes_neon<const TERM: u8>((mut x, mut y): Starts, chunks: usize) -> ByteLanes {
    let mut acc = [vdupq_n_s32(0); 4];
    for _ in 0..chunks {
        // SAFETY: a chunk the caller vouches for.
        let left = unsafe { widened(x) };
        // SAFETY: as above, of the other vector.
        let right = unsafe { widened(y) };
        for ((sum, p), q) in acc.iter_mut().zip(left).zip(right) {
            *sum = vaddq_s32(*sum, byte_term4::<TERM>(p, q));
        }
        (x, y) = (x.wrapping_add(BYTE_STEP), y.wrapping_add(BYTE_STEP));
    }
    let mut out = [0_i32; LANES];
    let at = out.as_mut_ptr();
    for (v, quarter) in acc.into_iter().zip(QUARTERS) {
        // SAFETY: `out` holds sixteen `i32`s; each store writes four of them.
        unsafe { vst1q_s32(at.wrapping_add(quarter), v) };
    }
    out
}

/// The `i32` lanes of `term` on NEON.
///
/// # Safety
///
/// As `bytes_neon`.
pub(super) unsafe fn i8_neon(term: DistanceTerm, starts: Starts, chunks: usize) -> ByteLanes {
    let body: unsafe fn(Starts, usize) -> ByteLanes = match term {
        DistanceTerm::Dot => bytes_neon::<DOT>,
        DistanceTerm::Square => bytes_neon::<SQUARE>,
        DistanceTerm::Gap => bytes_neon::<GAP>,
    };
    // SAFETY: the caller vouches for the processor and the chunks.
    unsafe { body(starts, chunks) }
}
