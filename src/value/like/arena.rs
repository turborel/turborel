//! \[DESIGN\] (RULES.md D1121) A batch's values matched all at once, and a
//! value matched by its bytes.
//!
//! A batch keeps its strings one after another in one arena, each row's
//! end noted (`storage::Texts`). A pattern of a piece between two `%`s
//! matched a value at a time searched each value alone, and a search of a
//! few dozen bytes spends much of itself starting and ending. So the
//! longest piece between is searched for in the whole arena by the
//! database's kernels (`Kernels::find_bytes`), which compare a vector of
//! places at a time. A place found is in the row whose end is the first
//! past it. If the piece ends past that row's end, it is not in the row.
//! Otherwise that row is matched whole by its pieces, and the search goes
//! on from the row's end. A row the search never stops in lacks the piece,
//! so no part of it can match.
//!
//! A pattern of no piece between, as `abc%`, compares the ends of a value
//! alone and gains nothing from the arena, and a pattern with `_` has no
//! pieces: each is matched a value at a time as before.
//!
//! A scan that decodes each value into a buffer of its own matches it by
//! its bytes (`BytesLike`), the pieces made once for its conjunct. A LIKE
//! with a collation never makes a `LikePattern`, as its matching is the
//! collation's (`like/collated.rs`).

use super::LikePattern;
use super::pieces::Pieces;
use crate::kernels::Kernels;

impl Pieces {
    /// Whether each value of the arena `text`, the `n`th ending at
    /// `ends[n]`, matches the pattern; none if it has no piece between.
    fn arena(&self, (text, ends): (&[u8], &[usize]), kernels: &dyn Kernels) -> Option<Vec<bool>> {
        let key = self.between.iter().max_by_key(|f| f.len())?;
        let mut matched = vec![false; ends.len()];
        let (mut row, mut from) = (0_usize, 0_usize);
        while let Some(found) = text.get(from..).and_then(|rest| kernels.find_bytes(rest, key.bytes())) {
            let at = from.saturating_add(found);
            row = row.saturating_add(ends.get(row..).map_or(0, |rest| rest.partition_point(|&end| end <= at)));
            let Some(&end) = ends.get(row) else { break };
            if at.saturating_add(key.len()) <= end {
                let start = row.checked_sub(1).and_then(|r| ends.get(r)).copied().unwrap_or(0);
                if let Some(slot) = matched.get_mut(row) {
                    *slot = text.get(start..end).is_some_and(|value| self.matches(value));
                }
            }
            from = end;
            row = row.saturating_add(1);
        }
        Some(matched)
    }
}

impl LikePattern {
    /// [X3.135-1986 §5.14 GR5] Whether each value of the arena `text`, the
    /// `n`th ending at `ends[n]`, matches it, the arena searched at once
    /// by `kernels`: none of a pattern with `_` or of no piece between two
    /// `%`s, which a value at a time matches as fast.
    pub(crate) fn matches_all(&self, values: (&[u8], &[usize]), kernels: &dyn Kernels) -> Option<Vec<bool>> {
        self.pieces.as_ref()?.arena(values, kernels)
    }

    /// Its match of a value's bytes, made once; none of a pattern with
    /// `_`, which is matched a character at a time.
    pub fn bytes(&self) -> Option<BytesLike> {
        self.pieces.clone().map(BytesLike)
    }
}

/// \[DESIGN\] A pattern of no `_` matched of a value's bytes, its pieces
/// each made once (RULES.md D1074, D1118).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytesLike(Pieces);

impl BytesLike {
    /// [X3.135-1986 §5.14 GR5] Whether the character string of the bytes
    /// `value` matches the pattern.
    pub fn matches(&self, value: &[u8]) -> bool {
        self.0.matches(value)
    }
}

// Unit tests: an arena's values matched as each alone (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernels::Portable;

    /// The arena of `values` and each one's end.
    fn arena(values: &[&str]) -> (Vec<u8>, Vec<usize>) {
        let mut text = Vec::new();
        let ends = values
            .iter()
            .map(|v| {
                text.extend_from_slice(v.as_bytes());
                text.len()
            })
            .collect();
        (text, ends)
    }

    /// Verifies: [DESIGN] RULES.md D1121: each value of an arena matches a
    /// pattern of a piece between as it matches alone: a piece found in no
    /// row, in the first and the last row, ending past its row's end into
    /// the next, found twice in a row, after empty rows, a row holding the
    /// longest piece but not the others or not in order, and a row whose
    /// ends hold the first and last pieces.
    #[test]
    fn an_arena_matches_as_each_value_alone() {
        let values = [
            "special requests",
            "",
            "spe",
            "cial requests",
            "requests special",
            "x special y requests z",
            "requestsrequests",
            "",
            "special",
            "nothing here",
            "ab special requests ba",
            "special requests special requests",
        ];
        let (text, ends) = arena(&values);
        for p in ["%special%requests%", "%special%", "%requests%", "ab%special%ba", "%cial r%", "%s%s%"] {
            let pattern = LikePattern::new(p, None).unwrap();
            let all = pattern.matches_all((&text, &ends), &Portable).unwrap_or_else(|| panic!("{p:?} has a piece between"));
            let alone: Vec<bool> = values.iter().map(|v| pattern.matches(v)).collect();
            assert_eq!(all, alone, "{p:?}");
            let bytes = pattern.bytes().unwrap_or_else(|| panic!("{p:?} has pieces"));
            assert_eq!(values.iter().map(|v| bytes.matches(v.as_bytes())).collect::<Vec<_>>(), alone, "{p:?} of bytes");
        }
        assert_eq!(LikePattern::new("%x%", None).unwrap().matches_all((&[], &[]), &Portable), Some(vec![]), "no rows");
    }

    /// Verifies: [DESIGN] RULES.md D1121: a pattern of no piece between, or
    /// with `_`, is not matched over the arena, and one with `_` has no
    /// match of bytes.
    #[test]
    fn a_pattern_of_no_piece_between_is_matched_alone() {
        let (text, ends) = arena(&["abc", "xabc"]);
        for p in ["abc%", "%abc", "abc", "%", "a%c", "%a_c%"] {
            assert_eq!(LikePattern::new(p, None).unwrap().matches_all((&text, &ends), &Portable), None, "{p:?}");
        }
        assert_eq!(LikePattern::new("%a_c%", None).unwrap().bytes(), None);
    }
}
