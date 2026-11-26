//! \[DESIGN\] The portable body of the substring search kernel (RULES.md
//! D1118, D1121), the reference the wider bodies are held to.
//!
//! It looks at eight places of the bytes at once. It reads the eight
//! bytes from a place and the eight bytes the needle's length less one
//! further on as two words, and marks each place whose byte is the
//! needle's first and whose byte that far on is the needle's last. Each
//! place marked is compared with the whole needle, so a mark is a
//! candidate and never a match by itself. The test of a byte of a word
//! for zero marks every zero byte, and a byte above a zero byte at times
//! too. A mark too many is compared and found wanting, and no place that
//! holds the needle goes unmarked. The places past the last whole word
//! are compared one at a time. The first place of the needle is found, as
//! a substring search finds it.

/// Each byte of a word 1.
const ONES: u64 = u64::from_le_bytes([1; 8]);

/// Each byte of a word its high bit.
const HIGHS: u64 = u64::from_le_bytes([0x80; 8]);

/// `Kernels::find_bytes`: the first place of `haystack` that holds
/// `needle`, if one does; 0 of an empty needle.
pub fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let (Some(&first), Some(&last)) = (needle.first(), needle.last()) else { return Some(0) };
    let (first, last) = (ONES.wrapping_mul(u64::from(first)), ONES.wrapping_mul(u64::from(last)));
    let reach = needle.len().saturating_sub(1);
    let mut at = 0_usize;
    while let (Some(ahead), Some(behind)) = (word(haystack, at), word(haystack, at.saturating_add(reach))) {
        let mut marks = zeros(ahead ^ first) & zeros(behind ^ last);
        while marks != 0 {
            let place = at.saturating_add(usize::try_from(marks.trailing_zeros() >> 3_u32).unwrap_or(0));
            if holds(haystack, place, needle) {
                return Some(place);
            }
            marks &= marks.wrapping_sub(1);
        }
        at = at.saturating_add(8);
    }
    let end = haystack.len().checked_sub(reach)?;
    (at..end).find(|&place| holds(haystack, place, needle))
}

/// Whether `haystack` holds `needle` at `place`.
pub fn holds(haystack: &[u8], place: usize, needle: &[u8]) -> bool {
    haystack.get(place..place.saturating_add(needle.len())) == Some(needle)
}

/// The eight bytes of `bytes` from `at`, as a word, if it has them.
fn word(bytes: &[u8], at: usize) -> Option<u64> {
    let eight: [u8; 8] = bytes.get(at..at.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(eight))
}

/// Each zero byte of `x` with its high bit set, and at times a byte above
/// one; no other.
fn zeros(x: u64) -> u64 {
    x.wrapping_sub(ONES) & !x & HIGHS
}

// Unit tests: the search against the standard library's (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D1118: a needle is found at the first
    /// place that holds it, as a search a place at a time finds it, or
    /// found nowhere: of one byte and of many, at the start, across and
    /// past a word's end, in the bytes past the last whole word, repeated
    /// and overlapping itself, of bytes alike but for their first or last,
    /// of characters of several bytes, of a zero byte and a byte above it
    /// in a word, in bytes shorter than the needle, and of an empty needle.
    #[test]
    fn a_needle_is_found_where_a_search_finds_it() {
        let needles = ["", "a", "ab", "aa", "aba", "special", "requests", "é", "€é", "\u{0}", "\u{1}a", "abcdefghij"];
        let mut haystacks: Vec<String> = [
            "",
            "a",
            "b",
            "ab",
            "aab",
            "abab",
            "xxxxxxxab",
            "xxxxxxxxab",
            "xxxxxxxxxxxxxxab",
            "aaaaaaaaaaaaaaaaaaaa",
            "furiously special packages wake; requests",
            "spec",
            "requestsrequest",
            "x€yéz",
            "\u{0}\u{1}a\u{0}",
            "abcdefghiabcdefghij",
        ]
        .iter()
        .map(|v| (*v).to_owned())
        .collect();
        haystacks.extend((0..24).map(|n| format!("{}aba{}", "z".repeat(n), "y".repeat(n % 9))));
        haystacks.extend((0..24).map(|n| format!("{}spexial specia special", "q".repeat(n))));
        for needle in needles {
            for haystack in &haystacks {
                assert_eq!(find_bytes(haystack.as_bytes(), needle.as_bytes()), haystack.find(needle), "{needle:?} in {haystack:?}");
            }
        }
    }

    /// Verifies: [DESIGN] RULES.md D1118: the test of a word for zero
    /// bytes marks each zero byte, never a byte below the first zero one.
    #[test]
    fn every_zero_byte_is_marked() {
        for at in 0..8 {
            let mut bytes = [0x41_u8; 8];
            bytes[at] = 0;
            let marks = zeros(u64::from_le_bytes(bytes));
            assert_eq!(marks.trailing_zeros() >> 3_u32, u32::try_from(at).unwrap_or(0), "{bytes:?}");
        }
        assert_eq!(zeros(u64::from_le_bytes([0x41; 8])), 0, "no zero byte");
    }
}
