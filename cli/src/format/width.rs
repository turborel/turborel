//! \[DESIGN\] How many columns of a terminal text takes (RULES.md D213),
//! measured from the Unicode 13.0 data turborel's regular expressions read
//! (`turborel::regex`, RULES.md D61): a character of a nonspacing or
//! enclosing mark, a format character or a control takes none; one of the
//! blocks of East Asian ideographs, syllables and pictographs takes two,
//! as do the fullwidth forms; any other takes one.

use std::collections::HashMap;

use turborel::regex::Regex;

/// The general categories of characters that take no column: nonspacing
/// and enclosing marks, format characters and controls.
const NONE_WIDE: &str = r"[\p{Mn}\p{Me}\p{Cf}\p{Cc}]";
/// The blocks whose characters take two columns, by their names in
/// Unicode 13.0's Blocks.txt, normalized [XSD 1.1 Part 2 §G.4.2.3].
const DOUBLE_BLOCKS: [&str; 33] = [
    "HangulJamo",
    "CJKRadicalsSupplement",
    "KangxiRadicals",
    "IdeographicDescriptionCharacters",
    "CJKSymbolsandPunctuation",
    "Hiragana",
    "Katakana",
    "Bopomofo",
    "HangulCompatibilityJamo",
    "Kanbun",
    "BopomofoExtended",
    "CJKStrokes",
    "KatakanaPhoneticExtensions",
    "EnclosedCJKLettersandMonths",
    "CJKCompatibility",
    "CJKUnifiedIdeographsExtensionA",
    "CJKUnifiedIdeographs",
    "YiSyllables",
    "YiRadicals",
    "HangulJamoExtended-A",
    "HangulSyllables",
    "CJKCompatibilityIdeographs",
    "VerticalForms",
    "CJKCompatibilityForms",
    "MiscellaneousSymbolsandPictographs",
    "Emoticons",
    "TransportandMapSymbols",
    "SupplementalSymbolsandPictographs",
    "SymbolsandPictographsExtended-A",
    "CJKUnifiedIdeographsExtensionB",
    "CJKUnifiedIdeographsExtensionC",
    "CJKUnifiedIdeographsExtensionD",
    "CJKCompatibilityIdeographsSupplement",
];
/// The fullwidth forms of the block Halfwidth and Fullwidth Forms, whose
/// halfwidth forms take one column [Unicode 13.0 Blocks.txt].
const FULLWIDTH: [(char, char); 2] = [('\u{FF01}', '\u{FF60}'), ('\u{FFE0}', '\u{FFE6}')];
/// The columns of a character of the double blocks.
const DOUBLE: usize = 2;

/// What measures text, each character's columns kept once found.
#[derive(Default)]
pub struct Widths {
    /// The classes, made once a character beyond ASCII is met.
    classes: Option<Option<(Regex, Regex)>>,
    found: HashMap<char, usize>,
}

impl Widths {
    /// The columns `text` takes.
    pub fn of(&mut self, text: &str) -> usize {
        text.chars().map(|c| self.char_width(c)).fold(0, usize::saturating_add)
    }

    /// The columns `c` takes.
    pub fn char_width(&mut self, c: char) -> usize {
        if c.is_ascii() {
            return usize::from(!c.is_ascii_control());
        }
        if let Some(&known) = self.found.get(&c) {
            return known;
        }
        let width = self.measured(c);
        self.found.insert(c, width);
        width
    }

    /// The columns `c`, beyond ASCII, takes, by its category and block;
    /// one if the classes cannot be made.
    fn measured(&mut self, c: char) -> usize {
        if FULLWIDTH.iter().any(|(first, last)| (*first..=*last).contains(&c)) {
            return DOUBLE;
        }
        let classes = self.classes.get_or_insert_with(classes);
        let Some((none, double)) = classes.as_ref() else { return 1 };
        let text = c.to_string();
        if none.is_match(&text).unwrap_or(false) {
            0
        } else if double.is_match(&text).unwrap_or(false) {
            DOUBLE
        } else {
            1
        }
    }

    /// The longest start of `text` that takes at most `columns`, and the
    /// rest: at least one character, so a character wider than `columns`
    /// still moves on.
    pub fn split_at_width<'t>(&mut self, text: &'t str, columns: usize) -> (&'t str, &'t str) {
        let mut taken = 0_usize;
        for (at, c) in text.char_indices() {
            let next = taken.saturating_add(self.char_width(c));
            if next > columns && at > 0 {
                return text.split_at(at);
            }
            taken = next;
        }
        (text, "")
    }

    /// `text` padded with spaces to `columns`, after it, or before it if
    /// `right`.
    pub fn pad(&mut self, text: &str, columns: usize, right: bool) -> String {
        let fill = " ".repeat(columns.saturating_sub(self.of(text)));
        if right { format!("{fill}{text}") } else { format!("{text}{fill}") }
    }
}

/// The classes of characters of no column and of two.
fn classes() -> Option<(Regex, Regex)> {
    let blocks: String = DOUBLE_BLOCKS.iter().map(|b| format!(r"\p{{Is{b}}}")).collect();
    Some((Regex::new(NONE_WIDE, "").ok()?, Regex::new(&format!("[{blocks}]"), "").ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D213: ASCII takes a column a character
    /// but controls none; a combining mark and a format character none;
    /// ideographs, syllables, pictographs and fullwidth forms two; a
    /// halfwidth form and other letters one.
    #[test]
    fn characters_take_their_columns() {
        let mut w = Widths::default();
        assert!(classes().is_some(), "every block is of Unicode 13.0");
        assert_eq!(w.of("abc"), 3);
        assert_eq!(w.of("a\tb"), 2);
        assert_eq!(w.of("e\u{301}"), 1, "a combining acute");
        assert_eq!(w.of("\u{200B}"), 0, "a zero width space");
        assert_eq!(w.of("日本"), 4);
        assert_eq!(w.of("한"), 2);
        assert_eq!(w.of("\u{1F600}"), 2, "an emoticon");
        assert_eq!(w.of("Ａ"), 2, "a fullwidth A");
        assert_eq!(w.of("ｱ"), 1, "a halfwidth katakana");
        assert_eq!(w.of("é"), 1);
        assert_eq!(w.of("é"), 1, "found once, kept");
    }

    /// Verifies: [DESIGN] RULES.md D213: text is split where its columns
    /// fill a width, a wide character never split, and one wider than the
    /// width still taken; text is padded by its columns.
    #[test]
    fn text_is_split_and_padded_by_columns() {
        let mut w = Widths::default();
        assert_eq!(w.split_at_width("abcdef", 4), ("abcd", "ef"));
        assert_eq!(w.split_at_width("日本語", 3), ("日", "本語"));
        assert_eq!(w.split_at_width("日", 1), ("日", ""));
        assert_eq!(w.split_at_width("", 1), ("", ""));
        assert_eq!(w.pad("日", 4, false), "日  ");
        assert_eq!(w.pad("ab", 4, true), "  ab");
        assert_eq!(w.pad("abcde", 4, true), "abcde");
    }
}
