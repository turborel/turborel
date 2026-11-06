//! \[EXTENSION\] [ISO/IEC 9075-2:2023 §4.3.6] The collation `UNICODE`, which
//! conforms to ISO/IEC 14651:2020 without delta: the Unicode Collation
//! Algorithm \[UTS #10\] at UCA 13.0.0, which that edition is built from,
//! with its Shifted option, to four levels, and no identical level, so
//! canonically equivalent strings compare equal.
//!
//! Under PAD SPACE, not ISO/IEC 14651's own, trailing spaces are taken away
//! first. A space is variable, so it weighs at the fourth level alone, and
//! there a character below a space orders as if it were past the string's
//! end (CONFORMANCE.md).

use std::sync::Arc;

use super::table::{Element, MOST_CONTRACTED, Table, VERSION};
use super::{Collation, Pad, UNICODE};

/// Bases of implicit primaries \[UTS #10 §10.1.3\].
const UNIFIED_BASE: u32 = 0xFB40;
const EXTENDED_BASE: u32 = 0xFB80;
const OTHER_BASE: u32 = 0xFBC0;
/// The low 15 bits of a code point go in the second element.
const LOW_BITS: u32 = 15;
const LOW_MASK: u32 = 0x7FFF;
/// The second element's flag.
const TRAILING: u32 = 0x8000;
/// The weights of an implicit primary's first element.
const COMMON_SECONDARY: u16 = 0x0020;
const COMMON_TERTIARY: u16 = 0x0002;
/// The fourth-level weight of what is not variable \[UTS #10 §4.4\].
const MAXIMUM: u16 = 0xFFFF;
/// [The Unicode Standard §3.12] Hangul syllables.
const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const S_COUNT: u32 = L_COUNT * V_COUNT * T_COUNT;
/// [Unicode 13.0 §18.1] The blocks of unified ideographs of the BMP.
const UNIFIED: [(u32, u32); 1] = [(0x4E00, 0x9FFC)];
/// The twelve unified ideographs in the compatibility block.
const COMPATIBILITY_UNIFIED: [u32; 12] = [0xFA0E, 0xFA0F, 0xFA11, 0xFA13, 0xFA14, 0xFA1F, 0xFA21, 0xFA23, 0xFA24, 0xFA27, 0xFA28, 0xFA29];
/// The extensions' blocks.
const EXTENSIONS: [(u32, u32); 7] = [
    (0x3400, 0x4DBF),
    (0x20000, 0x2A6DD),
    (0x2A700, 0x2B734),
    (0x2B740, 0x2B81D),
    (0x2B820, 0x2CEA1),
    (0x2CEB0, 0x2EBE0),
    (0x30000, 0x3134A),
];

/// `UNICODE`, reading `Table`.
pub struct Unicode(Arc<Table>);

impl Unicode {
    pub(super) fn new(table: Arc<Table>) -> Unicode {
        Unicode(table)
    }

    /// \[UTS #10 S1\] `text` in Normalization Form D.
    fn normalized(&self, text: &str) -> Vec<char> {
        let mut out = Vec::with_capacity(text.len());
        for c in text.chars() {
            let code = u32::from(c);
            match code.checked_sub(S_BASE).filter(|s| *s < S_COUNT) {
                Some(s) => hangul(s, &mut out),
                None => self.0.decompose(c, &mut out),
            }
        }
        self.reordered(&mut out);
        out
    }

    /// [The Unicode Standard §3.11] Each run of combining marks in
    /// canonical order.
    fn reordered(&self, chars: &mut [char]) {
        let mut from = 0;
        while from < chars.len() {
            let run = chars.get(from..).map_or(0, |rest| rest.iter().take_while(|c| self.0.class(**c) != 0).count());
            if let Some(marks) = from.checked_add(run).and_then(|to| chars.get_mut(from..to)) {
                marks.sort_by_key(|c| self.0.class(*c));
            }
            from = from.saturating_add(run.max(1));
        }
    }

    /// \[UTS #10 S2\] The collation elements of `chars`.
    fn elements(&self, mut chars: Vec<char>) -> Vec<Element> {
        let mut out = Vec::with_capacity(chars.len());
        let mut at = 0;
        while let Some(&first) = chars.get(at) {
            let Some(entry) = self.0.entry(first) else {
                implicit(&self.0, first, &mut out);
                at = at.saturating_add(1);
                continue;
            };
            if !entry.contracts {
                self.0.elements(entry.run, &mut out);
                at = at.saturating_add(1);
                continue;
            }
            at = self.contracted(&mut chars, at, &mut out);
        }
        out
    }

    /// \[UTS #10 S2.1\] The longest contraction at `at`, and what S2.1.1
    /// adds to it of the unblocked non-starters after it: its elements
    /// added to `out`, and where the next starts.
    fn contracted(&self, chars: &mut Vec<char>, at: usize, out: &mut Vec<Element>) -> usize {
        let mut length = MOST_CONTRACTED;
        let mut found = None;
        while length > 1 && found.is_none() {
            found = at.checked_add(length).and_then(|end| chars.get(at..end)).and_then(|s| self.0.contraction(s));
            if found.is_none() {
                length = length.saturating_sub(1);
            }
        }
        let mut matched: Vec<char> = chars.get(at..at.saturating_add(length.max(1))).unwrap_or_default().to_vec();
        // The next character to start from: what S2.1.1 adds is removed,
        // so it does not move this on.
        let after = at.saturating_add(matched.len());
        let (mut next, mut greatest) = (after, 0);
        while let Some(&c) = chars.get(next) {
            let class = self.0.class(c);
            if class == 0 {
                break;
            }
            if greatest < class && matched.len() < MOST_CONTRACTED {
                matched.push(c);
                if let Some(run) = self.0.contraction(&matched) {
                    found = Some(run);
                    chars.remove(next);
                    continue;
                }
                matched.pop();
            }
            greatest = greatest.max(class);
            next = next.saturating_add(1);
        }
        let single = chars.get(at).and_then(|c| self.0.entry(*c)).map(|e| e.run);
        if let Some(run) = found.or(single) {
            self.0.elements(run, out);
        }
        after
    }
}

/// [The Unicode Standard §3.12] Syllable `s`'s jamo.
fn hangul(s: u32, out: &mut Vec<char>) {
    let vowels_and_tails = V_COUNT * T_COUNT;
    let l = s.checked_div(vowels_and_tails).unwrap_or(0);
    let v = s.checked_rem(vowels_and_tails).and_then(|rest| rest.checked_div(T_COUNT)).unwrap_or(0);
    let t = s.checked_rem(T_COUNT).unwrap_or(0);
    let jamo = [L_BASE.saturating_add(l), V_BASE.saturating_add(v)];
    out.extend(jamo.into_iter().filter_map(char::from_u32));
    if t != 0 {
        out.extend(char::from_u32(T_BASE.saturating_add(t)));
    }
}

/// \[UTS #10 §10.1\] The elements of `c`, which has no weights of its own.
fn implicit(table: &Table, c: char, out: &mut Vec<Element>) {
    let code = u32::from(c);
    let within = |ranges: &[(u32, u32)]| ranges.iter().any(|&(first, last)| first <= code && code <= last);
    let (first, second) = if let Some(range) = table.implicit(code) {
        (range.primary, code.saturating_sub(range.origin) | TRAILING)
    } else {
        let base = if within(&UNIFIED) || COMPATIBILITY_UNIFIED.contains(&code) {
            UNIFIED_BASE
        } else if within(&EXTENSIONS) {
            EXTENDED_BASE
        } else {
            OTHER_BASE
        };
        (base.saturating_add(code >> LOW_BITS), (code & LOW_MASK) | TRAILING)
    };
    let weight = |w: u32| u16::try_from(w).unwrap_or(MAXIMUM);
    out.push(Element { weights: [weight(first), COMMON_SECONDARY, COMMON_TERTIARY], variable: false });
    out.push(Element { weights: [weight(second), 0, 0], variable: false });
}

/// \[UTS #10 §4.4\] `elements`' four levels under Shifted.
fn levels(elements: &[Element]) -> [Vec<u16>; 4] {
    let mut levels: [Vec<u16>; 4] = Default::default();
    let mut after_variable = false;
    for e in elements {
        let [l1, l2, l3] = e.weights;
        let shifted = e.variable;
        let ignored = l1 == 0 && after_variable;
        let fourth = match (shifted, l1 == 0, l2 == 0 && l3 == 0) {
            (true, _, _) => l1,
            (false, _, true) => 0,
            (false, true, false) if ignored => 0,
            (false, _, false) => MAXIMUM,
        };
        if !shifted && !ignored {
            let [first, second, third, _] = &mut levels;
            for (level, weight) in [(first, l1), (second, l2), (third, l3)] {
                if weight != 0 {
                    level.push(weight);
                }
            }
        }
        if fourth != 0
            && let Some(level) = levels.get_mut(3)
        {
            level.push(fourth);
        }
        if shifted {
            after_variable = true;
        } else if l1 != 0 {
            after_variable = false;
        }
    }
    levels
}

impl Collation for Unicode {
    fn name(&self) -> &str {
        UNICODE
    }

    fn version(&self) -> &str {
        VERSION
    }

    /// \[UTS #10 S3\] Each level's weights, a zero weight between levels.
    fn sort_key(&self, text: &str, pad: Pad, out: &mut Vec<u8>) {
        let text = match pad {
            Pad::Space => text.trim_end_matches(' '),
            Pad::None => text,
        };
        let [first, second, third, fourth] = levels(&self.elements(self.normalized(text)));
        for (n, level) in [first, second, third, fourth].iter().enumerate() {
            if n > 0 {
                out.extend_from_slice(&[0, 0]);
            }
            for weight in level {
                out.extend_from_slice(&weight.to_be_bytes());
            }
        }
    }

    /// Characters may contract or expand, so none compares alone.
    fn fold(&self, _: char) -> Option<char> {
        None
    }
}
