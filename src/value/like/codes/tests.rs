use super::*;
use crate::value::BytesLike;

/// A generator of numbers, fixed by its seed (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13_u32;
        self.0 ^= self.0 >> 7_u32;
        self.0 ^= self.0 << 17_u32;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next().checked_rem(u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)).unwrap_or(0)
    }
}

/// The bytes `codes` write by `symbols`, an escape writing the byte after it.
fn decoded(codes: &[u8], symbols: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = codes.iter();
    while let Some(&c) = rest.next() {
        if c == ESCAPE {
            out.extend(rest.next());
        } else {
            out.extend_from_slice(&symbols[usize::from(c)]);
        }
    }
    out
}

/// Up to ten codes of a table of `count` symbols, a fifth of them an
/// escape and a letter.
fn codes_of(rng: &mut Rng, count: usize) -> Vec<u8> {
    let mut codes = Vec::new();
    for _ in 0..rng.below(10) {
        if rng.below(5) == 0 {
            codes.extend([ESCAPE, b"abc"[rng.below(3)]]);
        } else {
            codes.push(u8::try_from(rng.below(count)).unwrap_or(0));
        }
    }
    codes
}

/// Verifies: [X3.135-1986 §5.14 GR5]; [DESIGN] RULES.md D1122: of random
/// tables of symbols of one to eight bytes of a few letters, and values
/// written by their codes and escapes, a pattern's machine matches each
/// value as its pieces match the decoded bytes: of no `%`, of a piece at
/// either end or both, of pieces between that repeat or overlap
/// themselves or span symbols, of `%` alone and doubled, and of the empty
/// pattern and value.
#[test]
fn codes_match_as_the_decoded_values() {
    let patterns =
        ["", "%", "%%", "ab", "ab%", "%ab", "%ab%", "a%b", "%aba%", "%aab%ab%", "ab%ba%ab", "%abcab%", "%a%b%c%", "%bb%", "c%", "%cc"];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..300_u32 {
        let symbols: Vec<Vec<u8>> = (0..rng.below(12).saturating_add(1))
            .map(|_| (0..rng.below(8).saturating_add(1)).map(|_| b"abc"[rng.below(3)]).collect())
            .collect();
        let table: Vec<&[u8]> = symbols.iter().map(Vec::as_slice).collect();
        for p in patterns {
            let pattern = LikePattern::new(p, None).unwrap();
            let mut machine = pattern.on_codes(&table).unwrap_or_else(|| panic!("{p:?}"));
            let bytes: BytesLike = pattern.bytes().unwrap_or_else(|| panic!("{p:?}"));
            for _ in 0..20_u32 {
                let codes = codes_of(&mut rng, symbols.len());
                let value = decoded(&codes, &symbols);
                assert_eq!(machine.matches(&codes), Some(bytes.matches(&value)), "{p:?} of {value:?}, round {round}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1122: codes that end in an escape, or a
/// code of no symbol, give no answer; a pattern with `_` has no machine.
#[test]
fn malformed_codes_give_no_answer() {
    let table: Vec<&[u8]> = vec![b"ab", b"c"];
    let mut machine = LikePattern::new("%zz%", None).unwrap().on_codes(&table).unwrap();
    assert_eq!(machine.matches(&[0, ESCAPE]), None);
    assert_eq!(machine.matches(&[0, 7]), None);
    assert_eq!(machine.matches(&[ESCAPE, b'z', ESCAPE, b'z']), Some(true));
    assert!(LikePattern::new("%a_b%", None).unwrap().on_codes(&table).is_none());
}
