//! \[DESIGN\] (RULES.md D1118) A piece of a pattern searched for in a
//! value, made once for the pattern. A search of the standard library's
//! made its searcher again for each value it searched, and for a filter
//! of fifteen million values that making was a third of the matching.
//!
//! A value's piece is searched for by the portable body of the substring
//! search kernel (`kernels/portable/find.rs`), eight places at a time; a
//! batch's values are searched all at once by a database's kernels
//! (`like/arena.rs`, RULES.md D1119).

use crate::kernels::portable::find::find_bytes;

/// A piece made to be searched for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finder {
    /// The piece's bytes, never empty.
    piece: Vec<u8>,
}

impl Finder {
    /// The finder of `piece`; none of an empty one.
    pub(super) fn of(piece: &str) -> Option<Finder> {
        (!piece.is_empty()).then(|| Finder { piece: piece.as_bytes().to_vec() })
    }

    /// The piece's length in bytes.
    pub(super) fn len(&self) -> usize {
        self.piece.len()
    }

    /// The piece's bytes.
    pub(super) fn bytes(&self) -> &[u8] {
        &self.piece
    }

    /// The first place of `value` that holds the piece, if one does.
    pub(super) fn find(&self, value: &[u8]) -> Option<usize> {
        find_bytes(value, &self.piece)
    }
}

// Unit tests: the finder against a search a place at a time (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D1118: a piece is found at the first
    /// place that holds it, as the standard library's search finds it, or
    /// found nowhere, and an empty piece makes no finder.
    #[test]
    fn a_piece_is_found_where_a_search_finds_it() {
        for piece in ["a", "aba", "special", "€é"] {
            let finder = Finder::of(piece).unwrap_or_else(|| panic!("{piece:?}"));
            assert_eq!((finder.len(), finder.bytes()), (piece.len(), piece.as_bytes()));
            for value in ["", "ab", "xxxxxxxxaba", "furiously special packages", "x€éz", "spec"] {
                assert_eq!(finder.find(value.as_bytes()), value.find(piece), "{piece:?} in {value:?}");
            }
        }
        assert_eq!(Finder::of(""), None, "an empty piece");
    }
}
