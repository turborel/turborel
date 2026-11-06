//! [ISO/IEC 9075-2:2023 §4.3.2] The sort key of a string under PAD SPACE,
//! whose comparison effectively extends the shorter with spaces.
//!
//! A key cannot hold endless spaces, so each run of them says what follows
//! it. The byte `SPACE` occurs nowhere else in a key, and the marker after
//! it orders a run by the character that ends it: one below a space first,
//! then the string's end, then one above. A longer run sorts later before
//! a low character, which it holds off, and earlier before a high one.
//!
//! The key is exact: two strings' keys compare as
//! `value::compare_strings` compares them (the tests of `collation.rs`).

/// The space's byte.
const SPACE: u8 = b' ';
/// A run of spaces before a character below a space.
const BEFORE_LOW: u8 = 0x00;
/// The string's end, which always closes a key.
const BEFORE_END: u8 = 0x80;
/// A run of spaces before a character above a space.
const BEFORE_HIGH: u8 = 0xFF;

/// `chars`, as a sort key under PAD SPACE, added to `out`.
pub(super) fn padded(chars: impl Iterator<Item = char>, out: &mut Vec<u8>) {
    let text: String = chars.collect();
    let mut buffer = [0; 4];
    let mut run = 0_u32;
    for c in text.trim_end_matches(' ').chars() {
        if c == ' ' {
            run = run.saturating_add(1);
            continue;
        }
        if run > 0 {
            run_of(run, c, out);
            run = 0;
        }
        out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
    }
    out.extend_from_slice(&[SPACE, BEFORE_END]);
}

/// The tag of `run` spaces before `next`, added to `out`.
fn run_of(run: u32, next: char, out: &mut Vec<u8>) {
    out.push(SPACE);
    if next < ' ' {
        out.push(BEFORE_LOW);
        out.extend_from_slice(&run.to_be_bytes());
    } else {
        out.push(BEFORE_HIGH);
        out.extend_from_slice(&(!run).to_be_bytes());
    }
}
