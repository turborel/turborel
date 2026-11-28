//! \[DESIGN\] Texts compressed by a table of their common substrings
//! (RULES.md D31), after FSST (Boncz, Neumann and Leis, "FSST: Fast Random
//! Access String Compression", VLDB 2020): of a column's texts in a frozen
//! row group, at most `MOST_SYMBOLS` symbols of 1 to 8 bytes, each written
//! as a one-byte code, and a byte no symbol matches at as `ESCAPE` and the
//! byte itself; so a value is decoded alone, by its codes, and a text of
//! many values alike in their parts, as names, addresses or paths are,
//! takes a part of its bytes. The table is chosen once of all the column's
//! texts in the group, about `SAMPLE` bytes of them in `BLOCKS` runs taken
//! evenly through them, in `ROUNDS` rounds, as FSST chooses it, and written
//! in each chunk: each round the texts are encoded with the table so far,
//! and the symbols and the pairs of symbols next to each other that save
//! the most bytes kept.
//!
//! ```text
//! FSST:  symbols u8 | each: length u8 | its bytes | (count + 1) × u32,
//!        where each value's codes begin and the last ends | the codes
//! ```

use std::cmp::Ordering;
use std::collections::HashMap;

use super::fields::Cells;
use super::{WORD, corrupt, read_word, word};
use crate::error::Result;

mod lengths;
mod stepped;
pub(super) use stepped::{Stepped, Steps};

/// \[DESIGN\] The most symbols of a table: 64 of 8 bytes take 577 with
/// their lengths and count, a fourteenth of a page of 8 KiB.
const MOST_SYMBOLS: usize = 64;
/// \[DESIGN\] The longest symbol.
pub(super) const LONGEST: usize = 8;
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
pub(super) fn encode(fields: Cells<'_>, table: &Table, out: &mut Vec<u8>) -> Result<()> {
    table.write(out);
    let mut codes = Vec::new();
    out.extend_from_slice(&word(0)?);
    for field in fields.iter() {
        if let Some(f) = field {
            table.encode(f, &mut codes);
        }
        out.extend_from_slice(&word(codes.len())?);
    }
    out.extend_from_slice(&codes);
    Ok(())
}

/// A table of symbols, read: each code's bytes as a word of 8, and its
/// length, so a code is decoded by a copy of 8 bytes and a cut. The
/// bytes of a word past its symbol's length are zero, so two tables of
/// the same symbols are equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        Decoder::of(self).decode(codes, out)
    }
}

/// \[DESIGN\] (RULES.md D1317) The octets of a block texts are decoded
/// into before they are added to a vector: 256, thirty-two of the longest
/// symbol. A page of 8 KiB that holds a hundred texts or more holds them
/// in about 80 octets of codes each, so nearly every such text is decoded
/// whole before it is copied out once, and a decoder made for one value
/// clears no more than this.
const BLOCK: usize = 256;
/// The octets of a block past which the next symbol may not fit, so the
/// block is added to the vector first.
const FULL: usize = BLOCK - LONGEST;

/// \[DESIGN\] (RULES.md D1317) Texts decoded of a table's codes through a
/// block. A code's word of 8 octets is stored whole at the block's place
/// and the place moves on by the symbol's length, so a code is one store
/// of a fixed width and no call, and the text is added to its vector by
/// one copy at its end, or a block at a time if it is longer. Decoding
/// straight onto the vector asked it for room a code and cut it back a
/// code. The block is kept from text to text of a chunk.
pub(super) struct Decoder<'s> {
    symbols: &'s Symbols,
    block: [u8; BLOCK],
}

impl<'s> Decoder<'s> {
    /// A decoder of the codes of `symbols`.
    pub(super) fn of(symbols: &'s Symbols) -> Decoder<'s> {
        Decoder { symbols, block: [0; BLOCK] }
    }

    /// Appends the text `codes` encode to `out`; none if they end in an
    /// escape, or a code is of no symbol. What was appended of a text
    /// refused is of no use.
    pub(super) fn decode(&mut self, codes: &[u8], out: &mut Vec<u8>) -> Option<()> {
        let (mut at, mut rest) = (0_usize, codes.iter());
        while let Some(&c) = rest.next() {
            if at > FULL {
                out.extend_from_slice(self.block.get(..at)?);
                at = 0;
            }
            if c == ESCAPE {
                *self.block.get_mut(at)? = *rest.next()?;
                at = at.saturating_add(1);
                continue;
            }
            let code = usize::from(c);
            if code >= self.symbols.count {
                return None;
            }
            let (word, len) = (self.symbols.words.get(code)?, usize::from(*self.symbols.lens.get(code)?));
            self.block.get_mut(at..at.saturating_add(LONGEST))?.copy_from_slice(word);
            at = at.saturating_add(len);
        }
        out.extend_from_slice(self.block.get(..at)?);
        Some(())
    }
}

impl Symbols {
    /// \[DESIGN\] (RULES.md D360) The order of the text `codes` encode
    /// against `constant`, each padded with spaces to the other's length,
    /// as `compare_strings` orders two strings: each symbol's bytes
    /// compared with the constant's in place, the first that differs
    /// deciding, so no text is decoded and most are told apart by their
    /// first code. UTF-8 orders as the characters it encodes, and a space
    /// is one byte, so bytes order the strings as their characters do.
    /// None if the codes end in an escape, or a code is of no symbol.
    pub(super) fn compare_padded(&self, codes: &[u8], constant: &[u8]) -> Option<Ordering> {
        let (mut codes, mut rest) = (codes.iter(), constant.iter());
        while let Some(&c) = codes.next() {
            let bytes = if c == ESCAPE {
                std::slice::from_ref(codes.next()?)
            } else {
                let code = usize::from(c);
                if code >= self.count {
                    return None;
                }
                self.words.get(code)?.get(..usize::from(*self.lens.get(code)?))?
            };
            for &b in bytes {
                let k = rest.next().copied().unwrap_or(PAD);
                if b != k {
                    return Some(b.cmp(&k));
                }
            }
        }
        Some(rest.find(|&&k| k != PAD).map_or(Ordering::Equal, |k| PAD.cmp(k)))
    }

    /// [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) The order of the text
    /// `codes` encode against `constant`, neither padded, a prefix the
    /// less, as a collation of NO PAD that orders strings as their octets
    /// do compares them: each symbol's bytes compared with the constant's
    /// in place, the first that differs deciding, so no text is decoded.
    /// None if the codes end in an escape, or a code is of no symbol.
    pub(super) fn compare_unpadded(&self, codes: &[u8], constant: &[u8]) -> Option<Ordering> {
        let (mut codes, mut rest) = (codes.iter(), constant.iter());
        while let Some(&c) = codes.next() {
            let bytes = if c == ESCAPE {
                std::slice::from_ref(codes.next()?)
            } else {
                let code = usize::from(c);
                if code >= self.count {
                    return None;
                }
                self.words.get(code)?.get(..usize::from(*self.lens.get(code)?))?
            };
            for &b in bytes {
                let Some(&k) = rest.next() else { return Some(Ordering::Greater) };
                if b != k {
                    return Some(b.cmp(&k));
                }
            }
        }
        Some(if rest.next().is_some() { Ordering::Less } else { Ordering::Equal })
    }
}

/// The byte a string is padded with as it is compared: a space.
const PAD: u8 = b' ';

/// A table of symbols as a chunk holds it: read of its bytes, or one read
/// before of the same bytes, held for the runs of a group (`prepared.rs`).
pub(super) enum Held<'a> {
    Read(Box<Symbols>),
    Given(&'a Symbols),
}

impl std::ops::Deref for Held<'_> {
    type Target = Symbols;

    fn deref(&self) -> &Symbols {
        match self {
            Held::Read(symbols) => symbols,
            Held::Given(symbols) => symbols,
        }
    }
}

/// A chunk's texts as codes, read in place: its table and the bytes it
/// takes, where each value's codes begin, and the codes.
pub(super) struct Coded<'a> {
    symbols: Held<'a>,
    table: usize,
    offsets: &'a [u8],
    codes: &'a [u8],
}

impl<'a> Coded<'a> {
    /// The texts of a chunk of `count` values `rest` holds, its table
    /// `given` if read before, with the bytes it takes, else read.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its table is malformed, or it is too short.
    pub(super) fn parse(rest: &'a [u8], count: usize, given: Option<(&'a Symbols, usize)>) -> Result<Coded<'a>> {
        let (symbols, table) = match given {
            Some((symbols, table)) => (Held::Given(symbols), table),
            None => {
                Symbols::read(rest).map(|(s, table)| (Held::Read(Box::new(s)), table)).ok_or_else(|| corrupt("its table is malformed"))?
            }
        };
        let len = count.checked_add(1).and_then(|n| n.checked_mul(WORD)).ok_or_else(|| corrupt("its count is too large"))?;
        let offsets = rest.get(table..table.saturating_add(len)).ok_or_else(|| corrupt("it is too short"))?;
        Ok(Coded { symbols, table, offsets, codes: rest.get(table.saturating_add(len)..).unwrap_or_default() })
    }

    /// Its table, and the bytes it takes, to be given when its bytes are
    /// read again.
    pub(super) fn table(&self) -> (Box<Symbols>, usize) {
        (Box::new(*self.symbols), self.table)
    }

    /// Where the codes of value `n` begin and end.
    fn between(&self, n: usize) -> Option<(usize, usize)> {
        read_word(self.offsets, n.checked_mul(WORD)?).ok().zip(read_word(self.offsets, n.checked_add(1)?.checked_mul(WORD)?).ok())
    }

    /// The text of value `n`, decoded.
    pub(super) fn text(&self, n: usize) -> Option<Vec<u8>> {
        let mut text = Vec::new();
        self.symbols.decode(self.codes_of(n)?, &mut text)?;
        Some(text)
    }

    /// The codes of value `n`.
    pub(super) fn codes_of(&self, n: usize) -> Option<&[u8]> {
        let (a, b) = self.between(n)?;
        self.codes.get(a..b)
    }

    /// \[DESIGN\] (RULES.md D1317) A decoder of its table's codes, kept
    /// for the texts a run reads of it.
    pub(super) fn decoder(&self) -> Decoder<'_> {
        Decoder::of(&self.symbols)
    }

    /// The order of value `n` against `constant`, as
    /// `Symbols::compare_unpadded` gives it.
    pub(super) fn compare_unpadded(&self, n: usize, constant: &[u8]) -> Option<Ordering> {
        let (a, b) = self.between(n)?;
        self.symbols.compare_unpadded(self.codes.get(a..b)?, constant)
    }

    /// \[DESIGN\] (RULES.md D360) The order of value `n` against
    /// `constant`, as `Symbols::compare_padded` gives it.
    pub(super) fn compare_padded(&self, n: usize, constant: &[u8]) -> Option<Ordering> {
        let (a, b) = self.between(n)?;
        self.symbols.compare_padded(self.codes.get(a..b)?, constant)
    }

    /// Its table of symbols.
    pub(super) fn symbols(&self) -> &Symbols {
        &self.symbols
    }

    /// \[DESIGN\] (RULES.md D1251) Whether value `n` matches the pattern
    /// of `steps`, made of this table, as `Steps::matches` tells it.
    pub(super) fn liked(&self, n: usize, steps: &Steps) -> Option<bool> {
        let (a, b) = self.between(n)?;
        steps.matches(self.codes.get(a..b)?)
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

    /// Verifies: [DESIGN] RULES.md D1317, texts decoded through a block:
    /// texts of every count of codes from none to past three blocks, of
    /// symbols of one, two and eight octets and of escaped octets in turn,
    /// decoded one after another by one decoder, are each the octets their
    /// codes spell, added after what the vector held, so a symbol or an
    /// escape that falls at a block's end is whole; and a text refused
    /// leaves the one after it right.
    #[test]
    fn texts_longer_than_a_block_are_decoded_whole() {
        let (symbols, _) = Symbols::read(&[3, 1, b'a', 2, b'b', b'c', 8, b'd', b'e', b'f', b'g', b'h', b'i', b'j', b'k']).unwrap();
        let spelled: [&[u8]; 4] = [b"a", b"bc", b"defghijk", b"!"];
        let mut decoder = Decoder::of(&symbols);
        let mut out = b"held".to_vec();
        for count in 0..3 * BLOCK {
            let (mut codes, mut want) = (Vec::new(), b"held".to_vec());
            for n in 0..count {
                let k = (n * 7 + count) % 4;
                if k == 3 {
                    codes.extend([ESCAPE, b'!']);
                } else {
                    codes.push(u8::try_from(k).unwrap());
                }
                want.extend_from_slice(spelled[k]);
            }
            out.truncate(4);
            assert_eq!(decoder.decode(&codes, &mut out), Some(()), "{count} codes");
            assert_eq!(out, want, "{count} codes");
        }
        assert!(decoder.decode(&[2, 2, 9], &mut out).is_none(), "no symbol 9");
        out.clear();
        assert_eq!((decoder.decode(&[1, 0], &mut out), out.as_slice()), (Some(()), b"bca".as_slice()), "after a text refused");
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
