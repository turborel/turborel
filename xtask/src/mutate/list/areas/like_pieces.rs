//! The mutations of a LIKE pattern matched by its pieces (RULES.md
//! R-TEST-9, D1074, D1118, D1119): pieces found apart, the last piece
//! past the first, the substring search, and a batch's values matched at
//! once.

use super::super::{Mutation, Package};

/// The test that compares pieces with the specifiers' own matching.
const AS_SPECIFIERS: &str = "value::like::pieces::tests::pieces_match_as_the_specifiers_do";

/// The portable body of the substring search.
const FIND: &str = "src/kernels/portable/find.rs";

/// The test that compares the portable search with the standard library's.
const FOUND: &str = "kernels::portable::find::tests::a_needle_is_found_where_a_search_finds_it";

/// Mechanisms whose removal must fail the LIKE predicate's requirements.
pub const LIKE_PIECES: [Mutation; 6] = [
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
        original: "let Some(mut rest) = after.strip_suffix(last.as_bytes()) else { return false };",
        replacement: "let Some(mut rest) = after.strip_suffix(last.as_bytes()).or(value.ends_with(last.as_bytes()).then_some(&b\"\"[..])) else { return false };",
        integration: &[],
        unit: &[AS_SPECIFIERS],
    },
    Mutation {
        name: "like-finder-marks-first-twice",
        package: Package::Core,
        breaks: "a finder marks a place by the piece's first byte at both ends, so a place holding the piece goes unmarked (RULES.md D1118)",
        file: FIND,
        original: "let mut marks = zeros(ahead ^ first) & zeros(behind ^ last);",
        replacement: "let mut marks = zeros(ahead ^ first) & zeros(behind ^ first);",
        integration: &[],
        unit: &[FOUND],
    },
    Mutation {
        name: "like-finder-tail-skipped",
        package: Package::Core,
        breaks: "a finder compares no place past the last whole word, so a piece near a value's end is not found (RULES.md D1118)",
        file: FIND,
        original: "(at..end).find(|&place| holds(haystack, place, needle))",
        replacement: "(at..end.min(at)).find(|&place| holds(haystack, place, needle))",
        integration: &[],
        unit: &[FOUND],
    },
    Mutation {
        name: "like-arena-found-is-matched",
        package: Package::Core,
        breaks: "a row of a batch that holds the longest piece between is taken as a match, its other pieces unmatched (RULES.md D1119)",
        file: "src/value/like/arena.rs",
        original: "*slot = text.get(start..end).is_some_and(|value| self.matches(value));",
        replacement: "*slot = text.get(start..end).is_some();",
        integration: &[],
        unit: &["value::like::arena::tests::an_arena_matches_as_each_value_alone"],
    },
    Mutation {
        name: "simd-find-last-byte-unmarked",
        package: Package::Simd,
        breaks: "the search on AVX2 marks a place by the needle's first byte at both ends, so a place holding it goes unmarked (RULES.md D1119)",
        file: "simd/src/x86/find.rs",
        original: "_mm256_cmpeq_epi8(behind, last)",
        replacement: "_mm256_cmpeq_epi8(behind, first)",
        integration: &[],
        unit: &["tests::every_available_set_keeps_the_contract"],
    },
];
