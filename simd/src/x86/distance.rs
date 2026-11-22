//! \[DESIGN\] Distance sums on AVX2 and AVX-512 (RULES.md D571) keep the
//! portable bodies' sixteen lanes, two registers of eight on AVX2 and one
//! of sixteen on AVX-512, over the same whole chunks, each lane summed
//! alone by a multiply or subtract and then an add, never fused, so every
//! lane rounds as the portable lane does. FLOAT16 widens by F16C or
//! AVX-512's conversion, exactly; INT8 widens to `i32` lanes. The lanes are
//! stored and the portable `finish_*` sums them and the tail, so the
//! result is the portable one bit for bit. A function of a set is called
//! only by an `Isa` of it, made only where the processor has it
//! (`x86::isa`).

use std::arch::x86_64::{
    __m256, __m256i, __m512, __m512i, _mm_loadu_si128, _mm_srli_si128, _mm256_abs_epi32, _mm256_add_epi32, _mm256_add_ps, _mm256_andnot_ps,
    _mm256_cvtepi8_epi32, _mm256_cvtph_ps, _mm256_loadu_ps, _mm256_loadu_si256, _mm256_mul_ps, _mm256_mullo_epi32, _mm256_set1_ps,
    _mm256_setzero_ps, _mm256_setzero_si256, _mm256_storeu_ps, _mm256_storeu_si256, _mm256_sub_epi32, _mm256_sub_ps, _mm512_abs_epi32,
    _mm512_abs_ps, _mm512_add_epi32, _mm512_add_ps, _mm512_cvtepi8_epi32, _mm512_cvtph_ps, _mm512_loadu_ps, _mm512_mul_ps,
    _mm512_mullo_epi32, _mm512_setzero_ps, _mm512_setzero_si512, _mm512_storeu_ps, _mm512_storeu_si512, _mm512_sub_epi32, _mm512_sub_ps,
};

use turborel::kernels::DistanceTerm;
use turborel::kernels::portable::distance::{ByteLanes, HALF_STEP, LANES, Lanes, SINGLE_STEP};

/// `DistanceTerm::Dot` as a body's constant.
const DOT: u8 = 0;
/// `DistanceTerm::Square` as a body's constant.
const SQUARE: u8 = 1;
/// `DistanceTerm::Gap` as a body's constant.
const GAP: u8 = 2;

/// The bytes of one `f32` lane register of eight.
const EIGHT_SINGLES: usize = 32;
/// The bytes of eight FLOAT16 elements.
const EIGHT_HALVES: usize = 16;
/// The bytes of a chunk of INT8 elements.
const BYTE_STEP: usize = LANES;

/// Two vectors' starts, each of whole chunks of sixteen elements.
pub(super) type Starts = (*const u8, *const u8);

// ---------------------------------------------------------------------------
// AVX2
// ---------------------------------------------------------------------------

/// Eight `f32`s at `p`, of FLOAT16 elements if `HALF`.
///
/// # Safety
///
/// The processor has AVX2 and F16C, and `p` points to 16 readable bytes if
/// `HALF`, else 32.
#[target_feature(enable = "avx2,f16c")]
unsafe fn load8<const HALF: bool>(p: *const u8) -> __m256 {
    if HALF {
        // SAFETY: the caller vouches for 16 bytes, and an unaligned load
        // reads exactly them.
        let halves = unsafe { _mm_loadu_si128(p.cast()) };
        _mm256_cvtph_ps(halves)
    } else {
        // SAFETY: the caller vouches for 32 bytes, and an unaligned load
        // reads exactly them.
        unsafe { _mm256_loadu_ps(p.cast()) }
    }
}

/// The term of eight pairs.
#[target_feature(enable = "avx2")]
fn term8<const TERM: u8>(p: __m256, q: __m256) -> __m256 {
    if TERM == DOT {
        _mm256_mul_ps(p, q)
    } else if TERM == SQUARE {
        let d = _mm256_sub_ps(p, q);
        _mm256_mul_ps(d, d)
    } else {
        // The sign bit cleared, as `f32::abs` clears it.
        _mm256_andnot_ps(_mm256_set1_ps(-0.0), _mm256_sub_ps(p, q))
    }
}

/// Two registers of eight lanes stored in order.
#[target_feature(enable = "avx2")]
fn stored8(lo: __m256, hi: __m256) -> Lanes {
    let mut out = [0.0_f32; LANES];
    let at = out.as_mut_ptr();
    // SAFETY: `out` holds sixteen `f32`s; each store writes eight of them.
    unsafe { _mm256_storeu_ps(at, lo) };
    // SAFETY: as above, the second eight.
    unsafe { _mm256_storeu_ps(at.wrapping_add(8), hi) };
    out
}

/// The lanes of `TERM` summed over `chunks` chunks from `starts`.
///
/// # Safety
///
/// The processor has AVX2 and F16C, and each start points to `chunks`
/// readable chunks of sixteen elements, FLOAT16 if `HALF`, else FLOAT32.
#[target_feature(enable = "avx2,f16c")]
unsafe fn lanes_avx2<const TERM: u8, const HALF: bool>((mut x, mut y): Starts, chunks: usize) -> Lanes {
    let (step, half) = if HALF { (HALF_STEP, EIGHT_HALVES) } else { (SINGLE_STEP, EIGHT_SINGLES) };
    let (mut lo, mut hi) = (_mm256_setzero_ps(), _mm256_setzero_ps());
    for _ in 0..chunks {
        let (x1, y1) = (x.wrapping_add(half), y.wrapping_add(half));
        // SAFETY: half a chunk the caller vouches for, its first.
        let p0 = unsafe { load8::<HALF>(x) };
        // SAFETY: as above, of the other vector.
        let q0 = unsafe { load8::<HALF>(y) };
        // SAFETY: as above, the chunk's second half.
        let p1 = unsafe { load8::<HALF>(x1) };
        // SAFETY: as above, of the other vector.
        let q1 = unsafe { load8::<HALF>(y1) };
        lo = _mm256_add_ps(lo, term8::<TERM>(p0, q0));
        hi = _mm256_add_ps(hi, term8::<TERM>(p1, q1));
        (x, y) = (x.wrapping_add(step), y.wrapping_add(step));
    }
    stored8(lo, hi)
}

/// The lanes of `term` on AVX2, of FLOAT16 elements if `half`.
///
/// # Safety
///
/// As `lanes_avx2`.
pub(super) unsafe fn floats_avx2(term: DistanceTerm, half: bool, starts: Starts, chunks: usize) -> Lanes {
    let body: unsafe fn(Starts, usize) -> Lanes = match (term, half) {
        (DistanceTerm::Dot, true) => lanes_avx2::<DOT, true>,
        (DistanceTerm::Dot, false) => lanes_avx2::<DOT, false>,
        (DistanceTerm::Square, true) => lanes_avx2::<SQUARE, true>,
        (DistanceTerm::Square, false) => lanes_avx2::<SQUARE, false>,
        (DistanceTerm::Gap, true) => lanes_avx2::<GAP, true>,
        (DistanceTerm::Gap, false) => lanes_avx2::<GAP, false>,
    };
    // SAFETY: the caller vouches for the processor and the chunks.
    unsafe { body(starts, chunks) }
}

/// The lanes of `a·a`, `b·b` and `a·b` over `chunks` chunks of `f32`s.
///
/// # Safety
///
/// The processor has AVX2 and F16C, and each start points to `chunks`
/// readable chunks of sixteen `f32`s.
#[target_feature(enable = "avx2,f16c")]
pub(super) unsafe fn cosine_avx2((mut x, mut y): Starts, chunks: usize) -> (Lanes, Lanes, Lanes) {
    let zero = _mm256_setzero_ps();
    let (mut aa, mut bb, mut ab) = ((zero, zero), (zero, zero), (zero, zero));
    for _ in 0..chunks {
        let (x1, y1) = (x.wrapping_add(EIGHT_SINGLES), y.wrapping_add(EIGHT_SINGLES));
        // SAFETY: half a chunk the caller vouches for, its first.
        let p0 = unsafe { load8::<false>(x) };
        // SAFETY: as above, of the other vector.
        let q0 = unsafe { load8::<false>(y) };
        // SAFETY: as above, the chunk's second half.
        let p1 = unsafe { load8::<false>(x1) };
        // SAFETY: as above, of the other vector.
        let q1 = unsafe { load8::<false>(y1) };
        aa = (_mm256_add_ps(aa.0, _mm256_mul_ps(p0, p0)), _mm256_add_ps(aa.1, _mm256_mul_ps(p1, p1)));
        bb = (_mm256_add_ps(bb.0, _mm256_mul_ps(q0, q0)), _mm256_add_ps(bb.1, _mm256_mul_ps(q1, q1)));
        ab = (_mm256_add_ps(ab.0, _mm256_mul_ps(p0, q0)), _mm256_add_ps(ab.1, _mm256_mul_ps(p1, q1)));
        (x, y) = (x.wrapping_add(SINGLE_STEP), y.wrapping_add(SINGLE_STEP));
    }
    (stored8(aa.0, aa.1), stored8(bb.0, bb.1), stored8(ab.0, ab.1))
}

/// The term of eight pairs of INT8 elements widened to `i32`, wrapping.
#[target_feature(enable = "avx2")]
fn byte_term8<const TERM: u8>(p: __m256i, q: __m256i) -> __m256i {
    if TERM == DOT {
        _mm256_mullo_epi32(p, q)
    } else if TERM == SQUARE {
        let d = _mm256_sub_epi32(p, q);
        _mm256_mullo_epi32(d, d)
    } else {
        _mm256_abs_epi32(_mm256_sub_epi32(p, q))
    }
}

/// The `i32` lanes of `TERM` over `chunks` chunks of sixteen INT8s.
///
/// # Safety
///
/// The processor has AVX2, and each start points to `chunks` readable
/// chunks of sixteen bytes.
#[target_feature(enable = "avx2")]
unsafe fn bytes_avx2<const TERM: u8>((mut x, mut y): Starts, chunks: usize) -> ByteLanes {
    let (mut lo, mut hi) = (_mm256_setzero_si256(), _mm256_setzero_si256());
    for _ in 0..chunks {
        // SAFETY: a chunk the caller vouches for, read exactly.
        let p = unsafe { _mm_loadu_si128(x.cast()) };
        // SAFETY: as above, of the other vector.
        let q = unsafe { _mm_loadu_si128(y.cast()) };
        let (p0, q0) = (_mm256_cvtepi8_epi32(p), _mm256_cvtepi8_epi32(q));
        let (p1, q1) = (_mm256_cvtepi8_epi32(_mm_srli_si128::<8>(p)), _mm256_cvtepi8_epi32(_mm_srli_si128::<8>(q)));
        lo = _mm256_add_epi32(lo, byte_term8::<TERM>(p0, q0));
        hi = _mm256_add_epi32(hi, byte_term8::<TERM>(p1, q1));
        (x, y) = (x.wrapping_add(BYTE_STEP), y.wrapping_add(BYTE_STEP));
    }
    let mut out = [0_i32; LANES];
    let at = out.as_mut_ptr();
    // SAFETY: `out` holds sixteen `i32`s; each store writes eight of them.
    unsafe { _mm256_storeu_si256(at.cast(), lo) };
    // SAFETY: as above, the second eight.
    unsafe { _mm256_storeu_si256(at.wrapping_add(8).cast(), hi) };
    out
}

/// The `i32` lanes of `term` on AVX2.
///
/// # Safety
///
/// As `bytes_avx2`.
pub(super) unsafe fn i8_avx2(term: DistanceTerm, starts: Starts, chunks: usize) -> ByteLanes {
    let body: unsafe fn(Starts, usize) -> ByteLanes = match term {
        DistanceTerm::Dot => bytes_avx2::<DOT>,
        DistanceTerm::Square => bytes_avx2::<SQUARE>,
        DistanceTerm::Gap => bytes_avx2::<GAP>,
    };
    // SAFETY: the caller vouches for the processor and the chunks.
    unsafe { body(starts, chunks) }
}

// ---------------------------------------------------------------------------
// AVX-512
// ---------------------------------------------------------------------------

/// Sixteen `f32`s at `p`, of FLOAT16 elements if `HALF`.
///
/// # Safety
///
/// The processor has AVX-512 F, and `p` points to 32 readable bytes if
/// `HALF`, else 64.
#[target_feature(enable = "avx512f")]
unsafe fn load16<const HALF: bool>(p: *const u8) -> __m512 {
    if HALF {
        // SAFETY: the caller vouches for 32 bytes, and an unaligned load
        // reads exactly them.
        let halves = unsafe { _mm256_loadu_si256(p.cast()) };
        _mm512_cvtph_ps(halves)
    } else {
        // SAFETY: the caller vouches for 64 bytes, and an unaligned load
        // reads exactly them.
        unsafe { _mm512_loadu_ps(p.cast()) }
    }
}

/// The term of sixteen pairs.
#[target_feature(enable = "avx512f")]
fn term16<const TERM: u8>(p: __m512, q: __m512) -> __m512 {
    if TERM == DOT {
        _mm512_mul_ps(p, q)
    } else if TERM == SQUARE {
        let d = _mm512_sub_ps(p, q);
        _mm512_mul_ps(d, d)
    } else {
        _mm512_abs_ps(_mm512_sub_ps(p, q))
    }
}

/// A register of sixteen lanes stored in order.
#[target_feature(enable = "avx512f")]
fn stored16(v: __m512) -> Lanes {
    let mut out = [0.0_f32; LANES];
    // SAFETY: `out` holds sixteen `f32`s, and the store writes exactly them.
    unsafe { _mm512_storeu_ps(out.as_mut_ptr(), v) };
    out
}

/// The lanes of `TERM` summed over `chunks` chunks from `starts`.
///
/// # Safety
///
/// The processor has AVX-512 F, and each start points to `chunks`
/// readable chunks of sixteen elements, FLOAT16 if `HALF`, else FLOAT32.
#[target_feature(enable = "avx512f")]
unsafe fn lanes_avx512<const TERM: u8, const HALF: bool>((mut x, mut y): Starts, chunks: usize) -> Lanes {
    let step = if HALF { HALF_STEP } else { SINGLE_STEP };
    let mut acc = _mm512_setzero_ps();
    for _ in 0..chunks {
        // SAFETY: a chunk the caller vouches for.
        let p = unsafe { load16::<HALF>(x) };
        // SAFETY: as above, of the other vector.
        let q = unsafe { load16::<HALF>(y) };
        acc = _mm512_add_ps(acc, term16::<TERM>(p, q));
        (x, y) = (x.wrapping_add(step), y.wrapping_add(step));
    }
    stored16(acc)
}

/// The lanes of `term` on AVX-512, of FLOAT16 elements if `half`.
///
/// # Safety
///
/// As `lanes_avx512`.
pub(super) unsafe fn floats_avx512(term: DistanceTerm, half: bool, starts: Starts, chunks: usize) -> Lanes {
    let body: unsafe fn(Starts, usize) -> Lanes = match (term, half) {
        (DistanceTerm::Dot, true) => lanes_avx512::<DOT, true>,
        (DistanceTerm::Dot, false) => lanes_avx512::<DOT, false>,
        (DistanceTerm::Square, true) => lanes_avx512::<SQUARE, true>,
        (DistanceTerm::Square, false) => lanes_avx512::<SQUARE, false>,
        (DistanceTerm::Gap, true) => lanes_avx512::<GAP, true>,
        (DistanceTerm::Gap, false) => lanes_avx512::<GAP, false>,
    };
    // SAFETY: the caller vouches for the processor and the chunks.
    unsafe { body(starts, chunks) }
}

/// The lanes of `a·a`, `b·b` and `a·b` over `chunks` chunks of `f32`s.
///
/// # Safety
///
/// The processor has AVX-512 F, and each start points to `chunks`
/// readable chunks of sixteen `f32`s.
#[target_feature(enable = "avx512f")]
pub(super) unsafe fn cosine_avx512((mut x, mut y): Starts, chunks: usize) -> (Lanes, Lanes, Lanes) {
    let (mut aa, mut bb, mut ab) = (_mm512_setzero_ps(), _mm512_setzero_ps(), _mm512_setzero_ps());
    for _ in 0..chunks {
        // SAFETY: a chunk the caller vouches for.
        let p = unsafe { load16::<false>(x) };
        // SAFETY: as above, of the other vector.
        let q = unsafe { load16::<false>(y) };
        aa = _mm512_add_ps(aa, _mm512_mul_ps(p, p));
        bb = _mm512_add_ps(bb, _mm512_mul_ps(q, q));
        ab = _mm512_add_ps(ab, _mm512_mul_ps(p, q));
        (x, y) = (x.wrapping_add(SINGLE_STEP), y.wrapping_add(SINGLE_STEP));
    }
    (stored16(aa), stored16(bb), stored16(ab))
}

/// The term of sixteen pairs of INT8 elements widened to `i32`, wrapping.
#[target_feature(enable = "avx512f")]
fn byte_term16<const TERM: u8>(p: __m512i, q: __m512i) -> __m512i {
    if TERM == DOT {
        _mm512_mullo_epi32(p, q)
    } else if TERM == SQUARE {
        let d = _mm512_sub_epi32(p, q);
        _mm512_mullo_epi32(d, d)
    } else {
        _mm512_abs_epi32(_mm512_sub_epi32(p, q))
    }
}

/// The `i32` lanes of `TERM` over `chunks` chunks of sixteen INT8s.
///
/// # Safety
///
/// The processor has AVX-512 F, and each start points to `chunks`
/// readable chunks of sixteen bytes.
#[target_feature(enable = "avx512f")]
unsafe fn bytes_avx512<const TERM: u8>((mut x, mut y): Starts, chunks: usize) -> ByteLanes {
    let mut acc = _mm512_setzero_si512();
    for _ in 0..chunks {
        // SAFETY: a chunk the caller vouches for, read exactly.
        let p = unsafe { _mm_loadu_si128(x.cast()) };
        // SAFETY: as above, of the other vector.
        let q = unsafe { _mm_loadu_si128(y.cast()) };
        acc = _mm512_add_epi32(acc, byte_term16::<TERM>(_mm512_cvtepi8_epi32(p), _mm512_cvtepi8_epi32(q)));
        (x, y) = (x.wrapping_add(BYTE_STEP), y.wrapping_add(BYTE_STEP));
    }
    let mut out = [0_i32; LANES];
    // SAFETY: `out` holds sixteen `i32`s, and the store writes exactly them.
    unsafe { _mm512_storeu_si512(out.as_mut_ptr().cast(), acc) };
    out
}

/// The `i32` lanes of `term` on AVX-512.
///
/// # Safety
///
/// As `bytes_avx512`.
pub(super) unsafe fn i8_avx512(term: DistanceTerm, starts: Starts, chunks: usize) -> ByteLanes {
    let body: unsafe fn(Starts, usize) -> ByteLanes = match term {
        DistanceTerm::Dot => bytes_avx512::<DOT>,
        DistanceTerm::Square => bytes_avx512::<SQUARE>,
        DistanceTerm::Gap => bytes_avx512::<GAP>,
    };
    // SAFETY: the caller vouches for the processor and the chunks.
    unsafe { body(starts, chunks) }
}
