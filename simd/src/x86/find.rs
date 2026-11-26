//! \[DESIGN\] The substring search on AVX2 and AVX-512 (RULES.md D1119),
//! the place found the one `portable::find::find_bytes` finds. Each step
//! loads the 32 or 64 bytes from a place and as many from the needle's
//! length less one further on, compares the first with the needle's first
//! byte and the second with its last, and keeps a bit for each place where
//! both are equal. Each place so marked is compared with the whole needle,
//! in order, and the first that holds it is the answer. The places past
//! the last whole step are searched by the portable body. A function of a
//! set is called only by an `Isa` of it, made only where the processor has
//! it (`x86::isa`).

use std::arch::x86_64::{
    _mm256_and_si256, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_set1_epi8, _mm512_cmpeq_epi8_mask,
    _mm512_loadu_si512, _mm512_set1_epi8,
};

use turborel::kernels::portable::find::{find_bytes, holds};

/// The `N` bytes of `bytes` from `at`, if it has them.
fn block<const N: usize>(bytes: &[u8], at: usize) -> Option<&[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

/// The first place from `at` on of `haystack` that holds `needle`, by the
/// portable body.
fn rest(haystack: &[u8], needle: &[u8], at: usize) -> Option<usize> {
    find_bytes(haystack.get(at..)?, needle).map(|found| at.saturating_add(found))
}

/// The first place of `marks`, places from `at`, that holds `needle`.
fn first_held(haystack: &[u8], needle: &[u8], at: usize, mut marks: u64) -> Option<usize> {
    while marks != 0 {
        let place = at.saturating_add(usize::try_from(marks.trailing_zeros()).unwrap_or(0));
        if holds(haystack, place, needle) {
            return Some(place);
        }
        marks &= marks.wrapping_sub(1);
    }
    None
}

/// `Kernels::find_bytes` on AVX2.
pub(super) fn find_avx2(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { avx2(haystack, needle) }
}

/// `Kernels::find_bytes` on AVX-512.
pub(super) fn find_avx512(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    // SAFETY: as `find_avx2`'s, of AVX-512.
    unsafe { avx512(haystack, needle) }
}

/// The search, 32 places a step.
#[target_feature(enable = "avx2")]
fn avx2(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let (Some(&first), Some(&last)) = (needle.first(), needle.last()) else { return find_bytes(haystack, needle) };
    let (first, last) = (_mm256_set1_epi8(first.cast_signed()), _mm256_set1_epi8(last.cast_signed()));
    let reach = needle.len().saturating_sub(1);
    let mut at = 0_usize;
    while let (Some(ahead), Some(behind)) = (block::<32>(haystack, at), block::<32>(haystack, at.saturating_add(reach))) {
        // SAFETY: `ahead` is 32 bytes, and an unaligned load reads exactly them.
        let ahead = unsafe { _mm256_loadu_si256(ahead.as_ptr().cast()) };
        // SAFETY: as of `ahead`, of `behind`.
        let behind = unsafe { _mm256_loadu_si256(behind.as_ptr().cast()) };
        let both = _mm256_and_si256(_mm256_cmpeq_epi8(ahead, first), _mm256_cmpeq_epi8(behind, last));
        let marks = u64::from(_mm256_movemask_epi8(both).cast_unsigned());
        if let Some(place) = first_held(haystack, needle, at, marks) {
            return Some(place);
        }
        at = at.saturating_add(32);
    }
    rest(haystack, needle, at)
}

/// The search, 64 places a step.
#[target_feature(enable = "avx512f,avx512bw")]
fn avx512(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let (Some(&first), Some(&last)) = (needle.first(), needle.last()) else { return find_bytes(haystack, needle) };
    let (first, last) = (_mm512_set1_epi8(first.cast_signed()), _mm512_set1_epi8(last.cast_signed()));
    let reach = needle.len().saturating_sub(1);
    let mut at = 0_usize;
    while let (Some(ahead), Some(behind)) = (block::<64>(haystack, at), block::<64>(haystack, at.saturating_add(reach))) {
        // SAFETY: `ahead` is 64 bytes, and an unaligned load reads exactly them.
        let ahead = unsafe { _mm512_loadu_si512(ahead.as_ptr().cast()) };
        // SAFETY: as of `ahead`, of `behind`.
        let behind = unsafe { _mm512_loadu_si512(behind.as_ptr().cast()) };
        let marks = _mm512_cmpeq_epi8_mask(ahead, first) & _mm512_cmpeq_epi8_mask(behind, last);
        if let Some(place) = first_held(haystack, needle, at, marks) {
            return Some(place);
        }
        at = at.saturating_add(64);
    }
    rest(haystack, needle, at)
}
