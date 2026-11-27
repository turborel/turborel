//! \[DESIGN\] UTF-8 judged on AVX2 (RULES.md D1175), 32 bytes a step, as
//! Keiser and Lemire, "Validating UTF-8 In Less Than One Instruction Per
//! Byte" (2021), §5 judge it: each byte's high nibble, the byte before's
//! high and low nibbles each look up a set of the errors their pair may
//! be, and the three sets' common bits are the pair's errors, but for
//! the third and fourth bytes of a character, which the bytes two and
//! three back say must be continuations. A step of no byte past 127
//! looks nothing up. Where a step's last bytes begin a character it does
//! not end, the next step must end it, and the last bytes of the text
//! must end every character, the bytes past them taken as zeros. The text
//! is judged as `str::from_utf8` judges it, the contract tests hold, and
//! only then given as a `str`. A function of the set is called only by an
//! `Isa` of it, made only where the processor has it (`x86::isa`).

use std::arch::x86_64::{
    __m256i, _mm_loadu_si128, _mm256_alignr_epi8, _mm256_and_si256, _mm256_broadcastsi128_si256, _mm256_loadu_si256, _mm256_movemask_epi8,
    _mm256_or_si256, _mm256_permute2x128_si256, _mm256_set1_epi8, _mm256_setr_epi8, _mm256_setzero_si256, _mm256_shuffle_epi8,
    _mm256_srli_epi16, _mm256_subs_epu8, _mm256_testz_si256, _mm256_xor_si256,
};

/// A lead byte and a byte not a continuation, or two leads.
const TOO_SHORT: u8 = 1;
/// A byte below 128 and a continuation.
const TOO_LONG: u8 = 1 << 1;
/// 0xE0 and a continuation below 0xA0.
const OVERLONG_3: u8 = 1 << 2;
/// 0xF4 and a continuation of 0x90 or more, or a lead past 0xF4.
const TOO_LARGE: u8 = 1 << 3;
/// 0xED and a continuation of 0xA0 or more.
const SURROGATE: u8 = 1 << 4;
/// 0xC0 or 0xC1 and a continuation.
const OVERLONG_2: u8 = 1 << 5;
/// A lead past 0xF4 and a continuation below 0x90, or 0xF0 and one
/// below 0x90: of the pair alone the two are not told apart.
const TOO_LARGE_1000: u8 = 1 << 6;
/// As `TOO_LARGE_1000`.
const OVERLONG_4: u8 = 1 << 6;
/// Two continuations.
const TWO_CONTS: u8 = 1 << 7;
/// What a first byte's low nibble leaves to the others.
const CARRY: u8 = TOO_SHORT | TOO_LONG | TWO_CONTS;

/// Of the byte before, by its high nibble.
const FIRST_HIGH: [u8; 16] = [
    TOO_LONG,
    TOO_LONG,
    TOO_LONG,
    TOO_LONG,
    TOO_LONG,
    TOO_LONG,
    TOO_LONG,
    TOO_LONG,
    TWO_CONTS,
    TWO_CONTS,
    TWO_CONTS,
    TWO_CONTS,
    TOO_SHORT | OVERLONG_2,
    TOO_SHORT,
    TOO_SHORT | OVERLONG_3 | SURROGATE,
    TOO_SHORT | TOO_LARGE | TOO_LARGE_1000 | OVERLONG_4,
];

/// Of the byte before, by its low nibble.
const FIRST_LOW: [u8; 16] = [
    CARRY | OVERLONG_3 | OVERLONG_2 | OVERLONG_4,
    CARRY | OVERLONG_2,
    CARRY,
    CARRY,
    CARRY | TOO_LARGE,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000 | SURROGATE,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
    CARRY | TOO_LARGE | TOO_LARGE_1000,
];

/// Of the byte itself, by its high nibble.
const SECOND_HIGH: [u8; 16] = [
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
    TOO_LONG | OVERLONG_2 | TWO_CONTS | OVERLONG_3 | TOO_LARGE_1000 | OVERLONG_4,
    TOO_LONG | OVERLONG_2 | TWO_CONTS | OVERLONG_3 | TOO_LARGE,
    TOO_LONG | OVERLONG_2 | TWO_CONTS | SURROGATE | TOO_LARGE,
    TOO_LONG | OVERLONG_2 | TWO_CONTS | SURROGATE | TOO_LARGE,
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
    TOO_SHORT,
];

/// The tables and running state of a judgement.
struct Judged {
    /// `FIRST_HIGH`, `FIRST_LOW` and `SECOND_HIGH`, in both lanes.
    tables: [__m256i; 3],
    /// The low nibble of each byte.
    nibble: __m256i,
    /// Each byte's errors found so far, none if 0.
    errors: __m256i,
    /// The step before, zeros before the first.
    before: __m256i,
    /// Of the step before, past 0 where its last bytes begin a character
    /// it does not end.
    open: __m256i,
}

/// `Kernels::utf8` on AVX2 and AVX-512.
pub(super) fn utf8_avx2(bytes: &[u8]) -> Option<&str> {
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    let valid = unsafe { avx2(bytes) };
    // SAFETY: `avx2` holds bytes UTF-8 only where `str::from_utf8` does,
    // as the contract tests hold it of every lead, continuation and their
    // errors at every place of a step and across steps.
    valid.then(|| unsafe { std::str::from_utf8_unchecked(bytes) })
}

/// `table`, in both lanes.
#[target_feature(enable = "avx2")]
fn lanes(table: &[u8; 16]) -> __m256i {
    // SAFETY: `table` is 16 bytes, and an unaligned load reads exactly them.
    _mm256_broadcastsi128_si256(unsafe { _mm_loadu_si128(table.as_ptr().cast()) })
}

/// Whether `bytes` are UTF-8.
#[target_feature(enable = "avx2")]
fn avx2(bytes: &[u8]) -> bool {
    let zero = _mm256_setzero_si256();
    let tables = [lanes(&FIRST_HIGH), lanes(&FIRST_LOW), lanes(&SECOND_HIGH)];
    let mut judged = Judged { tables, nibble: _mm256_set1_epi8(0x0F), errors: zero, before: zero, open: zero };
    let (steps, rest) = bytes.as_chunks::<32>();
    for step in steps {
        // SAFETY: `step` is 32 bytes, and an unaligned load reads exactly them.
        judged.step(unsafe { _mm256_loadu_si256(step.as_ptr().cast()) });
    }
    let mut last = [0_u8; 32];
    last.iter_mut().zip(rest).for_each(|(to, &from)| *to = from);
    // SAFETY: `last` is 32 bytes, and an unaligned load reads exactly them.
    judged.step(unsafe { _mm256_loadu_si256(last.as_ptr().cast()) });
    let errors = _mm256_or_si256(judged.errors, judged.open);
    _mm256_testz_si256(errors, errors) == 1
}

impl Judged {
    /// Judges the 32 bytes `input`, after `before`.
    #[target_feature(enable = "avx2")]
    fn step(&mut self, input: __m256i) {
        if _mm256_movemask_epi8(input) == 0_i32 {
            // A character the step before began and this does not end.
            self.errors = _mm256_or_si256(self.errors, self.open);
            self.open = _mm256_setzero_si256();
        } else {
            self.errors = _mm256_or_si256(self.errors, self.pairs(input));
            // Past 0 at the last three bytes of a lead of more bytes than are left.
            let (four, three, two, any) = (0xEF_u8.cast_signed(), 0xDF_u8.cast_signed(), 0xBF_u8.cast_signed(), -1_i8);
            let most = _mm256_setr_epi8(
                any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any, any,
                any, any, any, any, any, four, three, two,
            );
            self.open = _mm256_subs_epu8(input, most);
        }
        self.before = input;
    }

    /// The errors of each byte of `input` with those before it.
    #[target_feature(enable = "avx2")]
    fn pairs(&self, input: __m256i) -> __m256i {
        // The last 16 bytes before and the first 16 of `input`, so each
        // lane's bytes shifted in from the left are those before them.
        let joined = _mm256_permute2x128_si256::<0x21>(self.before, input);
        let one = _mm256_alignr_epi8::<15>(input, joined);
        let [first_high, first_low, second_high] = self.tables;
        let low = _mm256_and_si256(one, self.nibble);
        let special = _mm256_and_si256(
            _mm256_and_si256(_mm256_shuffle_epi8(first_high, self.high(one)), _mm256_shuffle_epi8(first_low, low)),
            _mm256_shuffle_epi8(second_high, self.high(input)),
        );
        let (two, three) = (_mm256_alignr_epi8::<14>(input, joined), _mm256_alignr_epi8::<13>(input, joined));
        // Past 127 where the byte two back leads three bytes or more, or
        // three back four.
        let third = _mm256_subs_epu8(two, _mm256_set1_epi8(0x60));
        let fourth = _mm256_subs_epu8(three, _mm256_set1_epi8(0x70));
        let continued = _mm256_and_si256(_mm256_or_si256(third, fourth), _mm256_set1_epi8(i8::MIN));
        _mm256_xor_si256(continued, special)
    }

    /// The high nibble of each byte of `v`.
    #[target_feature(enable = "avx2")]
    fn high(&self, v: __m256i) -> __m256i {
        _mm256_and_si256(_mm256_srli_epi16::<4>(v), self.nibble)
    }
}
