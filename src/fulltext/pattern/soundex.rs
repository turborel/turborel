//! SOUNDS LIKE [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §6.9]: which words
//! sound like a word is implementation-dependent [§6.9.5 Description 2].
//! Here two words sound alike when their American Soundex codes are equal,
//! the code of the U.S. National Archives: the first letter, then a digit
//! for each later consonant sound, three digits in all.
//!
//! - Letters are the ASCII letters of the normalized word, so `é` counts
//!   as `e`; other characters are passed over.
//! - b f p v are 1; c g j k q s x z are 2; d t are 3; l is 4; m n are 5;
//!   r is 6. Vowels and y are no digit but part letters of the same digit;
//!   h and w are no digit and part nothing.
//! - Letters of the same digit next to each other, the first letter
//!   included, give it once. The code is cut or padded with 0 to four
//!   characters.
//! - A word with no ASCII letter has no code, and sounds like itself only.

/// The characters of a Soundex code.
const CODE_LENGTH: usize = 4;
/// What pads a short code.
const PAD: char = '0';

/// What a letter is to Soundex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sound {
    /// A consonant, its digit.
    Digit(char),
    /// A vowel or y: no digit, and it parts letters of the same digit.
    Vowel,
    /// h or w: no digit, and it parts nothing.
    Silent,
}

/// What the lower-case ASCII letter `c` is; none for another character.
fn sound(c: char) -> Option<Sound> {
    match c {
        'b' | 'f' | 'p' | 'v' => Some(Sound::Digit('1')),
        'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => Some(Sound::Digit('2')),
        'd' | 't' => Some(Sound::Digit('3')),
        'l' => Some(Sound::Digit('4')),
        'm' | 'n' => Some(Sound::Digit('5')),
        'r' => Some(Sound::Digit('6')),
        'a' | 'e' | 'i' | 'o' | 'u' | 'y' => Some(Sound::Vowel),
        'h' | 'w' => Some(Sound::Silent),
        _ => None,
    }
}

/// The American Soundex code of `word`, as `R163`; none if it has no
/// ASCII letter.
pub(crate) fn soundex(word: &str) -> Option<String> {
    let mut letters = word.chars().map(|c| c.to_ascii_lowercase()).filter_map(|c| sound(c).map(|s| (c, s)));
    let (first, first_sound) = letters.next()?;
    let mut code = String::with_capacity(CODE_LENGTH);
    code.push(first.to_ascii_uppercase());
    let mut last = first_sound;
    for (_, s) in letters {
        match s {
            Sound::Digit(digit) => {
                if s != last && code.len() < CODE_LENGTH {
                    code.push(digit);
                }
                last = s;
            }
            Sound::Vowel => last = Sound::Vowel,
            Sound::Silent => {}
        }
    }
    while code.len() < CODE_LENGTH {
        code.push(PAD);
    }
    Some(code)
}

// Unit tests: the codes the U.S. National Archives gives as examples
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-2:2002 §6.9.5] the American Soundex codes
    /// of the standard examples: Robert and Rupert R163, Rubin R150,
    /// Ashcraft A261 (h parts nothing), Tymczak T522 (a vowel parts the
    /// same digit), Pfister P236 (a first letter's digit given once),
    /// Honeyman H555, Lee L000 (padded).
    #[test]
    fn standard_codes() {
        for (word, code) in [
            ("robert", "R163"),
            ("rupert", "R163"),
            ("rubin", "R150"),
            ("ashcraft", "A261"),
            ("ashcroft", "A261"),
            ("tymczak", "T522"),
            ("pfister", "P236"),
            ("honeyman", "H555"),
            ("lee", "L000"),
        ] {
            assert_eq!(soundex(word).as_deref(), Some(code), "{word}");
        }
    }

    /// Verifies: [ISO/IEC 13249-2:2002 §6.9.5] case does not count,
    /// characters not ASCII letters are passed over, and a word with no
    /// ASCII letter has no code.
    #[test]
    fn letters_only() {
        assert_eq!(soundex("ROBERT"), soundex("robert"));
        assert_eq!(soundex("o'brien").as_deref(), Some("O165"));
        assert_eq!(soundex("1984"), None);
        assert_eq!(soundex(""), None);
    }
}
