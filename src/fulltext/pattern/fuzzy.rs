//! FUZZY FORM OF [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §6.10]: which
//! words are spelled like a word is implementation-dependent [§6.10.5
//! Description 2]. Here a word is spelled like another within an edit
//! distance of `SHORT_EDITS`, or `LONG_EDITS` for a word of
//! `LONG_WORD_CHARS` characters or more, counted on the pattern's word.
//! The distance is the restricted Damerau–Levenshtein distance (optimal
//! string alignment) over characters of the normalized words: each
//! insertion, deletion, substitution or swap of two neighbours is one
//! edit, and no character is edited twice.

/// The edits allowed of a word shorter than `LONG_WORD_CHARS`.
const SHORT_EDITS: usize = 1;
/// The edits allowed of a longer word.
const LONG_EDITS: usize = 2;
/// The characters from which a word is long.
const LONG_WORD_CHARS: usize = 8;

/// The edits a word of the pattern `word` allows.
pub(crate) fn allowed(word: &[char]) -> usize {
    if word.len() >= LONG_WORD_CHARS { LONG_EDITS } else { SHORT_EDITS }
}

/// Whether `a` is within `edits` edits of `b`.
pub(crate) fn within(a: &[char], b: &[char], edits: usize) -> bool {
    a.len().abs_diff(b.len()) <= edits && distance(a, b) <= edits
}

/// The restricted Damerau–Levenshtein distance of `a` and `b`, by rows of
/// the usual table: `row[j]` is the distance of the first `i` characters of
/// `a` and the first `j` of `b`.
fn distance(a: &[char], b: &[char]) -> usize {
    let mut before: Vec<usize> = Vec::new();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut last_a: Option<char> = None;
    for (i, &ca) in a.iter().enumerate() {
        let mut row = Vec::with_capacity(previous.len());
        row.push(i.saturating_add(1));
        let mut last_b: Option<char> = None;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let deleted = previous.get(j.saturating_add(1)).map_or(usize::MAX, |d| d.saturating_add(1));
            let inserted = row.last().map_or(usize::MAX, |d| d.saturating_add(1));
            let replaced = previous.get(j).map_or(usize::MAX, |d| d.saturating_add(cost));
            let mut best = deleted.min(inserted).min(replaced);
            if last_a == Some(cb) && last_b == Some(ca) {
                let swapped = j.checked_sub(1).and_then(|k| before.get(k)).map_or(usize::MAX, |d| d.saturating_add(1));
                best = best.min(swapped);
            }
            row.push(best);
            last_b = Some(cb);
        }
        before = std::mem::replace(&mut previous, row);
        last_a = Some(ca);
    }
    previous.last().copied().unwrap_or(0)
}

// Unit tests: distances and the limits (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn d(a: &str, b: &str) -> usize {
        distance(&chars(a), &chars(b))
    }

    /// Verifies: [ISO/IEC 13249-2:2002 §6.10.5] the distance counts one
    /// edit for each insertion, deletion, substitution and swap of two
    /// neighbours, none for equal words, and is symmetric.
    #[test]
    fn edits_counted() {
        assert_eq!(d("fuzzy", "fuzzy"), 0);
        assert_eq!(d("fuzzy", "fuzy"), 1);
        assert_eq!(d("fuzy", "fuzzy"), 1);
        assert_eq!(d("fuzzy", "fizzy"), 1);
        assert_eq!(d("fuzzy", "fzuzy"), 1);
        assert_eq!(d("kitten", "sitting"), 3);
        assert_eq!(d("", "abc"), 3);
        assert_eq!(d("abc", ""), 3);
        assert_eq!(d("ca", "abc"), 3, "the restricted distance edits no character twice");
    }

    /// Verifies: [ISO/IEC 13249-2:2002 §6.10.5] a word of fewer than 8
    /// characters allows 1 edit and one of 8 or more 2; `within` holds at
    /// the limit and not past it, and refuses at once words whose lengths
    /// differ by more.
    #[test]
    fn limits_by_length() {
        assert_eq!(allowed(&chars("seventy")), SHORT_EDITS);
        assert_eq!(allowed(&chars("eighteen")), LONG_EDITS);
        assert!(within(&chars("color"), &chars("colour"), 1));
        assert!(!within(&chars("color"), &chars("kolar"), 1));
        assert!(within(&chars("standard"), &chars("standrads"), 2));
        assert!(!within(&chars("standard"), &chars("stnadrads"), 2));
        assert!(!within(&chars("a"), &chars("abc"), 1));
    }
}
