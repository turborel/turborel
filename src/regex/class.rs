//! Character classes [XSD 1.1 Part 2 §G.4]: sets of characters, as the
//! sorted ranges of their code points, and the sets the character class
//! escapes and the wildcard name [§G.4.2.5], with ISO/IEC 9075-2:2023 §9.28
//! GR9a's newlines.

/// The last code point.
const LAST: u32 = 0x10_FFFF;

/// [UTS #18 RL1.6] The characters a newline sequence is made of, alone
/// each one: LF, VT, FF, CR, NEL, LINE SEPARATOR and PARAGRAPH SEPARATOR.
pub(super) const NEWLINES: [char; 7] = ['\n', '\u{B}', '\u{C}', '\r', '\u{85}', '\u{2028}', '\u{2029}'];

/// [ISO/IEC 9075-2:2023 §9.28 GR9a ii] What `\s` matches one character
/// at a time: space, tab and the newlines.
const SPACES: [char; 9] = [' ', '\t', '\n', '\u{B}', '\u{C}', '\r', '\u{85}', '\u{2028}', '\u{2029}'];

/// [XML 1.0 Fifth Edition §2.3 \[4\]] NameStartChar, the characters `\i`
/// matches [XSD 1.1 Part 2 §G.4.2.5].
const NAME_START: [(u32, u32); 16] = [
    (0x3A, 0x3A),
    (0x41, 0x5A),
    (0x5F, 0x5F),
    (0x61, 0x7A),
    (0xC0, 0xD6),
    (0xD8, 0xF6),
    (0xF8, 0x2FF),
    (0x370, 0x37D),
    (0x37F, 0x1FFF),
    (0x200C, 0x200D),
    (0x2070, 0x218F),
    (0x2C00, 0x2FEF),
    (0x3001, 0xD7FF),
    (0xF900, 0xFDCF),
    (0xFDF0, 0xFFFD),
    (0x1_0000, 0xE_FFFF),
];

/// [XML 1.0 Fifth Edition §2.3 \[4a\]] What NameChar adds to
/// NameStartChar, for `\c`.
const NAME_MORE: [(u32, u32); 6] = [(0x2D, 0x2E), (0x30, 0x39), (0xB7, 0xB7), (0x300, 0x36F), (0x203F, 0x2040), (0x3A, 0x3A)];

/// A set of characters: the ranges of their code points, sorted, apart,
/// and each first no greater than its last.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CharSet {
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    /// The set of `ranges`, in any order, overlapping or not.
    pub(super) fn of(mut ranges: Vec<(u32, u32)>) -> CharSet {
        ranges.retain(|(first, last)| first <= last);
        ranges.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
        for (first, last) in ranges {
            let touching = out.last_mut().filter(|previous| first <= previous.1.saturating_add(1));
            if let Some(previous) = touching {
                previous.1 = previous.1.max(last);
            } else {
                out.push((first, last));
            }
        }
        CharSet { ranges: out }
    }

    /// The set of `c` alone.
    pub(super) fn single(c: char) -> CharSet {
        CharSet { ranges: vec![(u32::from(c), u32::from(c))] }
    }

    /// The set of each of `chars`.
    pub(super) fn chars(chars: &[char]) -> CharSet {
        CharSet::of(chars.iter().map(|&c| (u32::from(c), u32::from(c))).collect())
    }

    /// Its ranges.
    pub(super) fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    /// Its characters and `other`'s.
    pub(super) fn union(&self, other: &CharSet) -> CharSet {
        CharSet::of(self.ranges.iter().chain(&other.ranges).copied().collect())
    }

    /// Every character not in it.
    pub(super) fn complement(&self) -> CharSet {
        let mut out = Vec::with_capacity(self.ranges.len().saturating_add(1));
        let mut next = 0_u32;
        for &(first, last) in &self.ranges {
            if first > next {
                out.push((next, first.saturating_sub(1)));
            }
            next = last.saturating_add(1);
        }
        if next <= LAST {
            out.push((next, LAST));
        }
        CharSet { ranges: out }
    }

    /// Its characters not in `other`.
    pub(super) fn minus(&self, other: &CharSet) -> CharSet {
        self.intersection(&other.complement())
    }

    /// Its characters also in `other`.
    fn intersection(&self, other: &CharSet) -> CharSet {
        let (mut a, mut b) = (self.ranges.iter().peekable(), other.ranges.iter().peekable());
        let mut out = Vec::new();
        while let (Some(&&(f1, l1)), Some(&&(f2, l2))) = (a.peek(), b.peek()) {
            let (first, last) = (f1.max(f2), l1.min(l2));
            if first <= last {
                out.push((first, last));
            }
            if l1 < l2 {
                a.next();
            } else {
                b.next();
            }
        }
        CharSet { ranges: out }
    }

    /// Whether `c` is in it.
    pub(crate) fn contains(&self, c: char) -> bool {
        let code = u32::from(c);
        let at = self.ranges.partition_point(|&(_, last)| last < code);
        self.ranges.get(at).is_some_and(|&(first, _)| first <= code)
    }
}

/// [XSD 1.1 Part 2 §G.4.2.5] The set of the multi-character escape `\`
/// `letter`, but `\d`, `\D`, `\w` and `\W`, which a category gives
/// (`Classes::multi`); none if it names none of the others.
pub(super) fn fixed_escape(letter: char) -> Option<CharSet> {
    let name_start = || CharSet::of(NAME_START.to_vec());
    let name = || name_start().union(&CharSet::of(NAME_MORE.to_vec()));
    match letter {
        's' => Some(CharSet::chars(&SPACES)),
        'S' => Some(CharSet::chars(&SPACES).complement()),
        'i' => Some(name_start()),
        'I' => Some(name_start().complement()),
        'c' => Some(name()),
        'C' => Some(name().complement()),
        _ => None,
    }
}

/// The set the wildcard `.` matches one character at a time without the
/// flag `s`: every character but a newline's [§9.28 GR9a i].
pub(super) fn not_newline() -> CharSet {
    CharSet::chars(&NEWLINES).complement()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] a set's ranges are merged when they overlap or
    /// touch, its complement and difference hold the characters they
    /// should, and membership is found at each range's ends.
    #[test]
    fn sets_merge_and_subtract() {
        let s = CharSet::of(vec![(0x61, 0x63), (0x62, 0x66), (0x67, 0x67), (0x30, 0x2F)]);
        assert_eq!(s.ranges(), [(0x61, 0x67)]);
        assert!(s.contains('a') && s.contains('g') && !s.contains('h') && !s.contains('`'));
        let c = s.complement();
        assert_eq!(c.ranges(), [(0, 0x60), (0x68, LAST)]);
        assert_eq!(c.complement(), s);
        let d = s.minus(&CharSet::chars(&['c', 'd']));
        assert_eq!(d.ranges(), [(0x61, 0x62), (0x65, 0x67)]);
        assert!(CharSet::default().complement().contains('\u{10FFFF}'));
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR9a ii, iii] `\s`
    /// and `\S` split the characters by the newlines of UTS #18 RL1.6, a
    /// space and a tab; `\i` and `\c` are XML's name characters.
    #[test]
    fn fixed_escapes_9_28() {
        let s = fixed_escape('s').unwrap();
        assert!(s.contains('\u{85}') && s.contains('\u{2029}') && s.contains('\t') && !s.contains('a'));
        assert!(fixed_escape('S').unwrap().contains('a') && !fixed_escape('S').unwrap().contains(' '));
        assert!(fixed_escape('i').unwrap().contains('_') && !fixed_escape('i').unwrap().contains('1'));
        assert!(fixed_escape('c').unwrap().contains('1') && fixed_escape('C').unwrap().contains(' '));
        assert!(fixed_escape('d').is_none());
        assert!(!not_newline().contains('\r') && not_newline().contains('a'));
    }
}
