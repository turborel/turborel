//! The mutations of a LIKE pattern matched by its pieces (RULES.md
//! R-TEST-9, D1074, D1118): pieces found apart, the last piece past the
//! first, and a piece's finder.

use super::super::{Mutation, Package};

/// The test that compares pieces with the specifiers' own matching.
const AS_SPECIFIERS: &str = "value::like::pieces::tests::pieces_match_as_the_specifiers_do";

/// Mechanisms whose removal must fail the LIKE predicate's requirements.
pub const LIKE_PIECES: [Mutation; 4] = [
    Mutation {
        name: "like-pieces-overlap",
        package: Package::Core,
        breaks: "a piece of a pattern is found in the characters the piece before it took",
        file: "src/value/like/pieces.rs",
        original: "rest = rest.get(at.saturating_add(piece.len())..).unwrap_or_default();",
        replacement: "rest = rest.get(at.saturating_add(1)..).unwrap_or_default();",
        integration: &[],
        unit: &[AS_SPECIFIERS],
    },
    Mutation {
        name: "like-last-piece-in-first",
        package: Package::Core,
        breaks: "a value ends with a pattern's last piece in the characters its first piece took",
        file: "src/value/like/pieces.rs",
        original: "let Some(rest) = after.strip_suffix(last.as_str()) else { return false };",
        replacement: "let Some(rest) = after.strip_suffix(last.as_str()).or(value.ends_with(last.as_str()).then_some(\"\")) else { return false };",
        integration: &[],
        unit: &[AS_SPECIFIERS],
    },
    Mutation {
        name: "like-finder-marks-first-twice",
        package: Package::Core,
        breaks: "a finder marks a place by the piece's first byte at both ends, so a place holding the piece goes unmarked (RULES.md D1118)",
        file: "src/value/like/finder.rs",
        original: "let mut marks = zeros(ahead ^ self.first) & zeros(behind ^ self.last);",
        replacement: "let mut marks = zeros(ahead ^ self.first) & zeros(behind ^ self.first);",
        integration: &[],
        unit: &["value::like::finder::tests::a_piece_is_found_where_a_search_finds_it"],
    },
    Mutation {
        name: "like-finder-tail-skipped",
        package: Package::Core,
        breaks: "a finder compares no place past the last whole word, so a piece near a value's end is not found (RULES.md D1118)",
        file: "src/value/like/finder.rs",
        original: "(at..end).find(|&place| self.holds(value, place))",
        replacement: "(at..end.min(at)).find(|&place| self.holds(value, place))",
        integration: &[],
        unit: &["value::like::finder::tests::a_piece_is_found_where_a_search_finds_it"],
    },
];
