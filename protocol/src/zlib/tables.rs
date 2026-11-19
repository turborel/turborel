//! \[EXTENSION\] The constants of DEFLATE's format (RFC 1951 §3.2.5,
//! §3.2.6, §3.2.7, RULES.md D193), shared by the inflater and the deflater.

/// The end-of-block symbol of the literal/length alphabet.
pub(crate) const END_OF_BLOCK: u16 = 256;
/// The first length symbol.
pub(crate) const FIRST_LENGTH: u16 = 257;
/// Symbols of the literal/length alphabet, 0 to 285 used; 286 and 287
/// take part only in the fixed code.
pub(crate) const LITERALS: usize = 288;
/// Symbols of the distance alphabet, 0 to 29 used; 30 and 31 only in the
/// fixed code.
pub(crate) const DISTANCES: usize = 32;
/// Symbols of the code length alphabet.
pub(crate) const CODE_LENGTHS: usize = 19;
/// The longest code of the literal/length and distance codes.
pub(crate) const MAX_BITS: usize = 15;
/// The longest code of the code length code (three bits to say it).
pub(crate) const MAX_CODE_LENGTH_BITS: usize = 7;
/// Literal/length and distance symbols a dynamic header may count.
pub(crate) const MAX_LITERAL_SYMBOLS: usize = 286;
pub(crate) const MAX_DISTANCE_SYMBOLS: usize = 30;
/// The fewest of each a dynamic header counts: HLIT + 257, HDIST + 1,
/// HCLEN + 4.
pub(crate) const MIN_LITERAL_SYMBOLS: usize = 257;
pub(crate) const MIN_DISTANCE_SYMBOLS: usize = 1;
pub(crate) const MIN_CODE_LENGTH_SYMBOLS: usize = 4;
/// The shortest and longest match.
pub(crate) const MIN_MATCH: usize = 3;
pub(crate) const MAX_MATCH: usize = 258;
/// The window: the farthest a distance reaches back.
pub(crate) const WINDOW: usize = 32_768;

/// The code length symbols repeating: 16 the previous length 3 to 6
/// times, 17 a zero 3 to 10 times, 18 a zero 11 to 138 times.
pub(crate) const REPEAT_PREVIOUS: u16 = 16;
pub(crate) const REPEAT_ZERO: u16 = 17;
pub(crate) const REPEAT_ZERO_LONG: u16 = 18;

/// The order the code length code's lengths are sent in (§3.2.7).
pub(crate) const CODE_LENGTH_ORDER: [usize; CODE_LENGTHS] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Each length symbol's base length, from 257 (§3.2.5).
pub(crate) const LENGTH_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
/// Each length symbol's extra bits.
pub(crate) const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
/// Each distance symbol's base distance.
pub(crate) const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385,
    24577,
];
/// Each distance symbol's extra bits.
pub(crate) const DISTANCE_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// The fixed literal/length code's lengths (§3.2.6): 8 bits for 0-143,
/// 9 for 144-255, 7 for 256-279, 8 for 280-287.
pub(crate) fn fixed_literal_lengths() -> [u8; LITERALS] {
    let mut lengths = [8_u8; LITERALS];
    for (symbol, length) in lengths.iter_mut().enumerate() {
        *length = match symbol {
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    lengths
}

/// The fixed distance code's lengths: five bits each.
pub(crate) const FIXED_DISTANCE_LENGTH: u8 = 5;

/// The length symbol of a match `length` long, as an index into
/// `LENGTH_BASE`.
pub(crate) fn length_index(length: usize) -> usize {
    let length = u16::try_from(length).unwrap_or(u16::MAX);
    LENGTH_BASE.iter().rposition(|base| *base <= length).unwrap_or(0)
}

/// The distance symbol of a match `distance` back.
pub(crate) fn distance_index(distance: usize) -> usize {
    let distance = u16::try_from(distance).unwrap_or(u16::MAX);
    DISTANCE_BASE.iter().rposition(|base| *base <= distance).unwrap_or(0)
}
