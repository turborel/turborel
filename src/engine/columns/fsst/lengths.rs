//! \[DESIGN\] (RULES.md D1267) The octet length of a text coded by a
//! table of symbols, taken of its codes with no octet written. A code
//! stands for its symbol's octets, 1 to 8 of them, and an escape for the
//! one octet after it (`fsst.rs`), so the lengths of a text's codes sum
//! to the length of the text they decode to.

use super::{Coded, ESCAPE, Symbols};

impl Symbols {
    /// The octets of the text `codes` encode. None if they end in an
    /// escape or a code is of no symbol, as `decode` gives none of them.
    fn octets(&self, codes: &[u8]) -> Option<usize> {
        let (mut rest, mut octets) = (codes.iter(), 0_usize);
        while let Some(&c) = rest.next() {
            if c == ESCAPE {
                rest.next()?;
                octets = octets.saturating_add(1);
                continue;
            }
            let code = usize::from(c);
            if code >= self.count {
                return None;
            }
            octets = octets.saturating_add(usize::from(*self.lens.get(code)?));
        }
        Some(octets)
    }
}

impl Coded<'_> {
    /// \[DESIGN\] (RULES.md D1267) The octet length of value `n`, as
    /// `text` would decode it, none decoded. None if its codes are not
    /// within the chunk or are malformed.
    pub(in crate::engine::columns) fn octets(&self, n: usize) -> Option<usize> {
        let (a, b) = self.between(n)?;
        self.symbols.octets(self.codes.get(a..b)?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Table;
    use super::*;

    /// Verifies: [DESIGN] RULES.md D1267, FSST: the octets counted of a
    /// text's codes are the octets it decodes to, of texts the table was
    /// chosen of, of one it was not, of one all escapes, of characters of
    /// many octets and of the empty text; codes that end in an escape or
    /// name no symbol count as none, as they decode to none.
    #[test]
    fn a_texts_codes_count_the_octets_it_decodes_to() {
        let texts: Vec<String> = (0..300_u32).map(|k| format!("https://www.example{}.org/catalog/item-{k}.html", k % 40)).collect();
        let bytes: Vec<&[u8]> = texts.iter().map(String::as_bytes).collect();
        let table = Table::chosen(&bytes);
        let mut written = Vec::new();
        table.write(&mut written);
        let symbols = Symbols::read(&written).unwrap().0;
        for text in
            bytes.iter().take(40).copied().chain(["".as_bytes(), b"none of it", "h\u{e9}llo \u{6f22}\u{5b57}".as_bytes(), &[0, 254, 255]])
        {
            let mut codes = Vec::new();
            table.encode(text, &mut codes);
            assert_eq!(symbols.octets(&codes), Some(text.len()), "{text:?}");
        }
        assert!(written[0] > 0, "the table holds symbols");
        assert_eq!(symbols.octets(&[0, ESCAPE]), None, "an escape at the end");
        assert_eq!(symbols.octets(&[200]), None, "no symbol 200");
    }
}
