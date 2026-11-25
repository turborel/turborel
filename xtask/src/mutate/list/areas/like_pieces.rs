//! The mutations of a LIKE pattern matched by its pieces (RULES.md
//! R-TEST-9, D1074): pieces found apart, and the last piece past the
//! first.

use super::super::{Mutation, Package};

/// The test that compares pieces with the specifiers' own matching.
const AS_SPECIFIERS: &str = "value::like::pieces::tests::pieces_match_as_the_specifiers_do";

/// Mechanisms whose removal must fail the LIKE predicate's requirements.
pub const LIKE_PIECES: [Mutation; 2] = [
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
        original: "let Some(mut rest) = after.strip_suffix(last.as_str()) else { return false };",
        replacement: "let Some(mut rest) = after.strip_suffix(last.as_str()).or(value.ends_with(last.as_str()).then_some(\"\")) else { return false };",
        integration: &[],
        unit: &[AS_SPECIFIERS],
    },
];
