//! \[DESIGN\] Texts compressed by a table of their common substrings
//! (RULES.md D31), after FSST (Boncz, Neumann and Leis, "FSST: Fast Random
//! Access String Compression", VLDB 2020): of a chunk's texts, at most
//! `MOST_SYMBOLS` symbols of 1 to 8 bytes, each written as a one-byte
//! code, and a byte no symbol begins as `ESCAPE` and the byte itself; so a
//! value is decoded alone, by its codes, and a text of many values alike
//! in their parts, as names, addresses or paths are, takes a part of its
//! bytes. The table is chosen of the chunk's first values, `SAMPLE` bytes
//! of them, in `ROUNDS` rounds, as FSST chooses it: each round the texts
//! are encoded with the table so far, and the symbols and the pairs of
//! symbols next to each other that save the most bytes kept. Split from
//! encoded.rs (RULES.md R-SIZE-2).
//!
//! ```text
//! FSST:  symbols u8 | each: length u8 | its bytes | (count + 1) × u32,
//!        where each value's codes begin and the last ends | the codes
//! ```

use std::collections::HashMap;

use super::{WORD, corrupt, read_word, word};
use crate::error::Result;

/// \[DESIGN\] The most symbols of a table: 64 of 8 bytes take 576, a
/// fourteenth of a page of 8 KiB.
const MOST_SYMBOLS: usize = 64;
/// \[DESIGN\] The longest symbol.
const LONGEST: usize = 8;
/// The code of a byte written as itself, the next.
const ESCAPE: u8 = 255;
/// \[DESIGN\] The bytes of texts a table is chosen by.
const SAMPLE: usize = 16 << 10;
/// \[DESIGN\] The runs of texts a table is chosen of.
const BLOCKS: usize = 16;
/// \[DESIGN\] The rounds a table is chosen in, as FSST's.
const ROUNDS: usize = 5;

/// \[DESIGN\] A table of symbols, each its bytes, by code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Table {
    symbols: Vec<Vec<u8>>,
    /// The codes of the symbols that begin with each byte, the longest
    /// first.
    by_first: Vec<Vec<u8>>,
}

impl Table {
    /// The table of `symbols`.
    fn of(symbols: Vec<Vec<u8>>) -> Table {
        let mut by_first: Vec<Vec<u8>> = vec![Vec::new(); 256];
        for (code, s) in symbols.iter().enumerate() {
            let place = s.first().and_then(|&first| by_first.get_mut(usize::from(first)));
            if let (Some(codes), Ok(code)) = (place, u8::try_from(code)) {
                codes.push(code);
            }
        }
        for codes in &mut by_first {
            codes.sort_by_key(|&c| std::cmp::Reverse(symbols.get(usize::from(c)).map_or(0, Vec::len)));
        }
        Table { symbols, by_first }
    }

    /// The code of the longest symbol `text` begins with, and its length;
    /// none if no symbol is.
    fn longest(&self, text: &[u8]) -> Option<(u8, usize)> {
        let codes = self.by_first.get(usize::from(*text.first()?))?;
        codes.iter().find_map(|&c| {
            let s = self.symbols.get(usize::from(c))?;
            text.starts_with(s).then_some((c, s.len()))
        })
    }

    /// Each code of `text` as `each` is given it: a symbol's code and
    /// length, or none and the byte escaped.
    fn codes(&self, text: &[u8], mut each: impl FnMut(Option<u8>, &[u8])) {
        let mut at = 0_usize;
        while let Some(rest) = text.get(at..).filter(|r| !r.is_empty()) {
            let (code, len) = self.longest(rest).map_or((None, 1), |(code, len)| (Some(code), len));
            each(code, rest.get(..len).unwrap_or_default());
            at = at.saturating_add(len);
        }
    }

    /// The bytes `text` encodes in.
    pub(super) fn encoded_len(&self, text: &[u8]) -> usize {
        let mut len = 0_usize;
        self.codes(text, |code, _| len = len.saturating_add(if code.is_some() { 1 } else { 2 }));
        len
    }

    /// Appends `text`, encoded.
    pub(super) fn encode(&self, text: &[u8], out: &mut Vec<u8>) {
        self.codes(text, |code, bytes| {
            if let Some(c) = code {
                out.push(c);
            } else {
                out.push(ESCAPE);
                out.extend_from_slice(bytes);
            }
        });
    }

    /// The bytes the table takes written.
    pub(super) fn bytes(&self) -> usize {
        self.symbols.iter().map(|s| s.len().saturating_add(1)).fold(1, usize::saturating_add)
    }

    /// Appends the table, written.
    pub(super) fn write(&self, out: &mut Vec<u8>) {
        out.push(u8::try_from(self.symbols.len()).unwrap_or(u8::MAX));
        for s in &self.symbols {
            out.push(u8::try_from(s.len()).unwrap_or(u8::MAX));
            out.extend_from_slice(s);
        }
    }

    /// \[DESIGN\] The table chosen of `texts`, about `SAMPLE` bytes of them,
    /// as FSST chooses it: of `BLOCKS` runs of them one after another, the
    /// runs taken evenly through them, so texts that come in a pattern are
    /// sampled whole, each kind of them, as every n-th would not be.
    pub(super) fn chosen(texts: &[&[u8]]) -> Table {
        let block = SAMPLE.checked_div(BLOCKS).unwrap_or(SAMPLE);
        let mut sample: Vec<&[u8]> = Vec::new();
        for b in 0..BLOCKS {
            let start = texts.len().saturating_mul(b).checked_div(BLOCKS).unwrap_or(0);
            let mut taken = 0_usize;
            for t in texts.iter().skip(start).take_while(|t| {
                let more = taken < block;
                taken = taken.saturating_add(t.len());
                more
            }) {
                sample.push(t);
            }
        }
        sample.dedup_by(|a, b| std::ptr::eq(*a, *b));
        let mut table = Table::default();
        for _ in 0..ROUNDS {
            table = table.refined(&sample);
        }
        table
    }

    /// The table of the symbols that save the most bytes of `sample`
    /// encoded with this one, each its length for each time it is found:
    /// each symbol and each byte escaped, and each two next to each other
    /// joined, if of 8 bytes at most.
    fn refined(&self, sample: &[&[u8]]) -> Table {
        let mut gains: HashMap<Packed, usize> = HashMap::new();
        let mut add = |symbol: Packed| {
            let gain = gains.entry(symbol).or_insert(0);
            *gain = gain.saturating_add(usize::from(symbol.1));
        };
        let mut parts: Vec<Packed> = Vec::new();
        for text in sample {
            parts.clear();
            self.codes(text, |_, bytes| parts.push(packed(bytes)));
            for pair in parts.windows(2) {
                if let Some(both) = pair.first().zip(pair.get(1)).and_then(|(&before, &after)| joined(before, after)) {
                    add(both);
                }
            }
            parts.iter().copied().for_each(&mut add);
        }
        let mut ranked: Vec<(Packed, usize)> = gains.into_iter().collect();
        // The most saved first, then the longer, then by their bytes, so
        // the table chosen is the same of the same texts.
        ranked.sort_by(|(a, ga), (b, gb)| gb.cmp(ga).then(b.1.cmp(&a.1)).then(a.cmp(b)));
        Table::of(ranked.into_iter().take(MOST_SYMBOLS).map(|(symbol, _)| unpacked(symbol)).collect())
    }
}

/// A symbol of at most 8 bytes, as their bits and its length, so it is
/// counted with no allocation.
type Packed = (u64, u8);

fn packed(bytes: &[u8]) -> Packed {
    let mut word = [0_u8; LONGEST];
    let len = bytes.len().min(LONGEST);
    if let (Some(to), Some(from)) = (word.get_mut(..len), bytes.get(..len)) {
        to.copy_from_slice(from);
    }
    (u64::from_le_bytes(word), u8::try_from(len).unwrap_or(0))
}

fn unpacked((bits, len): Packed) -> Vec<u8> {
    bits.to_le_bytes().get(..usize::from(len)).unwrap_or_default().to_vec()
}

/// `before` then `after`, if they are 8 bytes at most.
fn joined((a, a_len): Packed, (b, b_len): Packed) -> Option<Packed> {
    let len = a_len.checked_add(b_len).filter(|&n| usize::from(n) <= LONGEST)?;
    let shifted = b.checked_shl(u32::from(a_len).checked_mul(8)?).unwrap_or(0);
    Some((a | shifted, len))
}

/// Appends `fields`, texts, encoded by `table`: the table, where each
/// value's codes begin, and the codes.
///
/// # Errors
///
/// SQLCODE -900 if they are longer than a count describes.
pub(super) fn encode(fields: &[Option<Vec<u8>>], table: &Table, out: &mut Vec<u8>) -> Result<()> {
    table.write(out);
    let mut codes = Vec::new();
    out.extend_from_slice(&word(0)?);
    for field in fields {
        if let Some(f) = field {
            table.encode(f, &mut codes);
        }
        out.extend_from_slice(&word(codes.len())?);
    }
    out.extend_from_slice(&codes);
    Ok(())
}

/// A table of symbols, read: each code's bytes as a word of 8, and its
/// length, so a code is decoded by a copy of 8 bytes and a cut.
#[derive(Debug, Clone, Copy)]
pub(super) struct Symbols {
    words: [[u8; LONGEST]; MOST_SYMBOLS],
    lens: [u8; MOST_SYMBOLS],
    count: usize,
}

impl Symbols {
    /// The table `bytes` begin with, and the bytes it takes; none if they
    /// are too short, of too many symbols, or of one too long or empty.
    pub(super) fn read(bytes: &[u8]) -> Option<(Symbols, usize)> {
        let count = usize::from(*bytes.first()?);
        if count > MOST_SYMBOLS {
            return None;
        }
        let (mut words, mut lens) = ([[0_u8; LONGEST]; MOST_SYMBOLS], [0_u8; MOST_SYMBOLS]);
        let mut next = 1_usize;
        for (word, slot) in words.iter_mut().zip(lens.iter_mut()).take(count) {
            let len = *bytes.get(next)?;
            if len == 0 || usize::from(len) > LONGEST {
                return None;
            }
            let from = next.checked_add(1)?;
            next = from.checked_add(usize::from(len))?;
            word.get_mut(..usize::from(len))?.copy_from_slice(bytes.get(from..next)?);
            *slot = len;
        }
        Some((Symbols { words, lens, count }, next))
    }

    /// Appends the text `codes` encode; none if they end in an escape, or
    /// a code is of no symbol.
    pub(super) fn decode(&self, codes: &[u8], out: &mut Vec<u8>) -> Option<()> {
        let mut rest = codes.iter();
        while let Some(&c) = rest.next() {
            if c == ESCAPE {
                out.push(*rest.next()?);
                continue;
            }
            let code = usize::from(c);
            if code >= self.count {
                return None;
            }
            let (word, len) = (self.words.get(code)?, usize::from(*self.lens.get(code)?));
            out.extend_from_slice(word);
            out.truncate(out.len().saturating_sub(LONGEST.saturating_sub(len)));
        }
        Some(())
    }
}

/// A chunk's texts as codes, read in place: its table, where each value's
/// codes begin, and the codes.
pub(super) struct Coded<'a> {
    symbols: Box<Symbols>,
    offsets: &'a [u8],
    codes: &'a [u8],
}

impl<'a> Coded<'a> {
    /// The texts of a chunk of `count` values `rest` holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its table is malformed, or it is too short.
    pub(super) fn parse(rest: &'a [u8], count: usize) -> Result<Coded<'a>> {
        let (symbols, table) = Symbols::read(rest).ok_or_else(|| corrupt("its table is malformed"))?;
        let len = count.checked_add(1).and_then(|n| n.checked_mul(WORD)).ok_or_else(|| corrupt("its count is too large"))?;
        let offsets = rest.get(table..table.saturating_add(len)).ok_or_else(|| corrupt("it is too short"))?;
        Ok(Coded { symbols: Box::new(symbols), offsets, codes: rest.get(table.saturating_add(len)..).unwrap_or_default() })
    }

    /// Where the codes of value `n` begin and end.
    fn between(&self, n: usize) -> Option<(usize, usize)> {
        read_word(self.offsets, n.checked_mul(WORD)?).ok().zip(read_word(self.offsets, n.checked_add(1)?.checked_mul(WORD)?).ok())
    }

    /// The text of value `n`, decoded.
    pub(super) fn text(&self, n: usize) -> Option<Vec<u8>> {
        let (a, b) = self.between(n)?;
        let mut text = Vec::new();
        self.symbols.decode(self.codes.get(a..b)?, &mut text)?;
        Some(text)
    }

    /// The texts at positions `from` to `to`, pushed to `text` and where
    /// each ends to `ends`, of those `null` says are not nulls: decoded one
    /// after another, then checked UTF-8 at once, each one's end on a
    /// character's boundary, so each is UTF-8 alone.
    pub(super) fn texts(
        &self,
        (from, to): (usize, usize),
        (text, ends): (&mut String, &mut Vec<usize>),
        null: impl Fn(usize) -> bool,
    ) -> Option<()> {
        let (base, mut decoded) = (text.len(), Vec::new());
        for n in from..to {
            if !null(n) {
                let (a, b) = self.between(n)?;
                self.symbols.decode(self.codes.get(a..b)?, &mut decoded)?;
            }
            ends.push(base.checked_add(decoded.len())?);
        }
        let decoded = std::str::from_utf8(&decoded).ok()?;
        let start = ends.len().checked_sub(to.checked_sub(from)?)?;
        if !ends.get(start..)?.iter().all(|&end| decoded.is_char_boundary(end.checked_sub(base).unwrap_or(usize::MAX))) {
            return None;
        }
        text.push_str(decoded);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texts alike in their parts, as `n` makes them.
    fn texts(n: u32) -> Vec<String> {
        (0..n)
            .map(|k| match k % 3 {
                0 => format!("https://www.example{}.org/catalog/item-{k}.html", k % 40),
                1 => format!("customer{k}@mail{}.example.com", k % 25),
                _ => format!("Street {} of the Avenue, Springfield {}", k % 300, k % 17),
            })
            .collect()
    }

    /// `text` encoded by `table`, then decoded by it as written.
    fn round_trip(table: &Table, text: &[u8]) -> Vec<u8> {
        let mut codes = Vec::new();
        table.encode(text, &mut codes);
        assert_eq!(codes.len(), table.encoded_len(text));
        let mut written = Vec::new();
        table.write(&mut written);
        assert_eq!(written.len(), table.bytes());
        let mut back = Vec::new();
        Symbols::read(&written).unwrap().0.decode(&codes, &mut back).unwrap();
        back
    }

    /// Verifies: [DESIGN] RULES.md D31, FSST: a table chosen of texts alike
    /// in their parts holds at most 64 symbols of at most 8 bytes and takes
    /// a quarter of their bytes or less; each text, and texts it was not
    /// chosen of, empty, of characters of many bytes, of bytes no symbol
    /// begins, read back as written; the same texts choose the same table.
    #[test]
    fn texts_read_back_from_their_codes() {
        let all = texts(3000);
        let bytes: Vec<&[u8]> = all.iter().map(String::as_bytes).collect();
        let table = Table::chosen(&bytes);
        assert!(table.symbols.len() <= MOST_SYMBOLS && table.symbols.iter().all(|s| !s.is_empty() && s.len() <= LONGEST));
        let plain: usize = bytes.iter().map(|t| t.len()).sum();
        let packed: usize = bytes.iter().map(|t| table.encoded_len(t)).sum();
        assert!(packed.saturating_mul(4) < plain, "{packed} of {plain}");
        for text in bytes.iter().take(30).copied().chain(["".as_bytes(), "héllo wörld ✓ 漢字".as_bytes(), &[0, 1, 254, 9]]) {
            assert_eq!(round_trip(&table, text), text);
        }
        assert_eq!(Table::chosen(&bytes), table, "the same table");
        assert_eq!(round_trip(&Table::default(), b"no symbols"), b"no symbols", "every byte escaped");
        assert!(Table::chosen(&[]).symbols.is_empty(), "of no texts");
    }

    /// Verifies: [DESIGN] RULES.md D31, FSST: a table of too many symbols,
    /// of one empty or longer than 8 bytes, or cut short, is not read; codes
    /// ending in an escape, or of a symbol not there, are not decoded.
    #[test]
    fn malformed_tables_and_codes_are_refused() {
        assert!(Symbols::read(&[65]).is_none(), "65 symbols");
        assert!(Symbols::read(&[1, 0]).is_none(), "empty");
        assert!(Symbols::read(&[1, 9, 1, 2, 3, 4, 5, 6, 7, 8, 9]).is_none(), "of 9 bytes");
        assert!(Symbols::read(&[2, 1, 97]).is_none(), "cut short");
        assert!(Symbols::read(&[]).is_none());
        let (symbols, len) = Symbols::read(&[1, 2, 97, 98]).unwrap();
        assert_eq!(len, 4);
        let mut out = Vec::new();
        assert_eq!(symbols.decode(&[0, ESCAPE, 99], &mut out), Some(()));
        assert_eq!(out, b"abc");
        assert!(symbols.decode(&[ESCAPE], &mut out).is_none(), "an escape at the end");
        assert!(symbols.decode(&[1], &mut out).is_none(), "no symbol 1");
    }
}
