//! \[DESIGN\] (RULES.md D1118) A piece of a pattern searched for in a
//! value, made once for the pattern. A search of the standard library's
//! made its searcher again for each value it searched, and for a filter
//! of fifteen million values that making was a third of the matching.
//!
//! The finder looks at eight places of the value at once. It reads the
//! eight bytes from a place and the eight bytes a piece's length less one
//! further on as two words, and marks each place whose byte is the
//! piece's first and whose byte that far on is the piece's last. Each
//! place marked is compared with the whole piece, so a mark is a
//! candidate and never a match by itself. The test of a byte of a word
//! for zero marks every zero byte, and a byte above a zero byte at times
//! too. A mark too many is compared and found wanting, and no place that
//! holds the piece goes unmarked. The places past the last whole word are
//! compared one at a time. The first place of the piece is found, as a
//! substring search finds it.

/// Each byte of a word 1.
const ONES: u64 = u64::from_le_bytes([1; 8]);

/// Each byte of a word its high bit.
const HIGHS: u64 = u64::from_le_bytes([0x80; 8]);

/// A piece made to be searched for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finder {
    /// The piece's bytes, never empty.
    piece: Vec<u8>,
    /// Its first byte, in each byte of a word.
    first: u64,
    /// Its last byte, in each byte of a word.
    last: u64,
}

impl Finder {
    /// The finder of `piece`; none of an empty one.
    pub(super) fn of(piece: &str) -> Option<Finder> {
        let bytes = piece.as_bytes();
        let (first, last) = (*bytes.first()?, *bytes.last()?);
        Some(Finder { piece: bytes.to_vec(), first: ONES.wrapping_mul(u64::from(first)), last: ONES.wrapping_mul(u64::from(last)) })
    }

    /// The piece's length in bytes.
    pub(super) fn len(&self) -> usize {
        self.piece.len()
    }

    /// The first place of `value` that holds the piece, if one does.
    pub(super) fn find(&self, value: &[u8]) -> Option<usize> {
        let reach = self.piece.len().checked_sub(1)?;
        let mut at = 0_usize;
        while let (Some(ahead), Some(behind)) = (word(value, at), word(value, at.saturating_add(reach))) {
            let mut marks = zeros(ahead ^ self.first) & zeros(behind ^ self.last);
            while marks != 0 {
                let place = at.saturating_add(usize::try_from(marks.trailing_zeros() >> 3_u32).unwrap_or(0));
                if self.holds(value, place) {
                    return Some(place);
                }
                marks &= marks.wrapping_sub(1);
            }
            at = at.saturating_add(8);
        }
        let end = value.len().checked_sub(reach)?;
        (at..end).find(|&place| self.holds(value, place))
    }

    /// Whether `value` holds the piece at `place`.
    fn holds(&self, value: &[u8], place: usize) -> bool {
        value.get(place..place.saturating_add(self.piece.len())) == Some(self.piece.as_slice())
    }
}

/// The eight bytes of `value` from `at`, as a word, if it has them.
fn word(value: &[u8], at: usize) -> Option<u64> {
    let bytes: [u8; 8] = value.get(at..at.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

/// Each zero byte of `x` with its high bit set, and at times a byte above
/// one; no other.
fn zeros(x: u64) -> u64 {
    x.wrapping_sub(ONES) & !x & HIGHS
}

// Unit tests: the finder against a search a place at a time (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D1118: a piece is found at the first
    /// place that holds it, as a search a place at a time finds it, or
    /// found nowhere: of one byte and of many, at the start, across and
    /// past a word's end, in the bytes past the last whole word, repeated
    /// and overlapping itself, of bytes alike but for their first or last,
    /// of characters of several bytes, of a zero byte and a byte above it
    /// in a word, and in a value shorter than the piece.
    #[test]
    fn a_piece_is_found_where_a_search_finds_it() {
        let pieces = ["a", "ab", "aa", "aba", "special", "requests", "é", "€é", "\u{0}", "\u{1}a", "abcdefghij"];
        let mut values: Vec<String> = [
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
        values.extend((0..24).map(|n| format!("{}aba{}", "z".repeat(n), "y".repeat(n % 9))));
        values.extend((0..24).map(|n| format!("{}spexial specia special", "q".repeat(n))));
        for piece in pieces {
            let finder = Finder::of(piece).unwrap_or_else(|| panic!("{piece:?}"));
            assert_eq!(finder.len(), piece.len());
            for value in &values {
                assert_eq!(finder.find(value.as_bytes()), value.find(piece), "{piece:?} in {value:?}");
            }
        }
        assert_eq!(Finder::of(""), None, "an empty piece");
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
