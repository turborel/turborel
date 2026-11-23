//! \[DESIGN\] (RULES.md D960) A character string's value [X3.135-1986
//! §4.2.1], held with no allocation where it is short and shared where it
//! is long.
//!
//! A string of at most `INLINE` octets of UTF-8 is held in the value
//! itself. A longer one is one buffer, never changed, that every clone of
//! the value shares. Either form is valid UTF-8, shown once where it is
//! made, so its octets are read with no check; the text of the form held
//! in the value is shown valid again each time it is asked for as a `str`,
//! over the whole array, which the alignment of the array makes cheap.

use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

/// The octets of UTF-8 a string held in the value takes at most: the
/// octets of a `Value` past its tag, less this form's tag and length, cut
/// to the two blocks of sixteen that a check of an aligned array reads
/// (RULES.md D960).
pub const INLINE: usize = 32;

/// The octets a string padded is built in on the stack before its one
/// buffer is made: a CHARACTER of more than `INLINE` and no more than
/// this many octets padded takes one allocation.
const STACKED: usize = 256;

/// The pad character's octet in UTF-8 [X3.135-1986 §5.11 GR5].
const SPACE: u8 = b' ';

/// The octets of a string held in the value, those past its length zero.
/// Aligned as a word is, so that a check of the whole array reads it a
/// block at a time.
#[derive(Clone, Copy)]
#[repr(align(8))]
struct Held([u8; INLINE]);

/// How a string is held.
#[derive(Clone)]
enum Form {
    /// In the value: `len` octets of valid UTF-8, then zeros.
    Inline {
        /// The octets of the string.
        len: u8,
        /// The string, then zeros.
        bytes: Held,
    },
    /// In a buffer its clones share.
    Shared(Arc<str>),
}

/// A character string's value: valid UTF-8, never changed in place.
#[derive(Clone)]
pub struct CharString(Form);

/// `octets` in the form held in a value, if they are few enough. Whether
/// they are UTF-8 is its caller's to show.
fn held(octets: &[u8]) -> Option<Form> {
    let mut bytes = [0_u8; INLINE];
    bytes.get_mut(..octets.len())?.copy_from_slice(octets);
    Some(Form::Inline { len: u8::try_from(octets.len()).ok()?, bytes: Held(bytes) })
}

impl CharString {
    /// The string of no characters.
    pub const fn new() -> CharString {
        CharString(Form::Inline { len: 0, bytes: Held([0; INLINE]) })
    }

    /// The string `octets` are the UTF-8 of; none if they are not UTF-8.
    /// The one check a string read from a record takes.
    pub fn from_utf8(octets: &[u8]) -> Option<CharString> {
        std::str::from_utf8(octets).ok().map(CharString::from)
    }

    /// Its octets as UTF-8, read with no check.
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            Form::Inline { len, bytes } => bytes.0.get(..usize::from(*len)).unwrap_or_default(),
            Form::Shared(s) => s.as_bytes(),
        }
    }

    /// Its text. Of a string held in the value, the whole array is shown
    /// to be UTF-8 first: its zeros are, so it is exactly when the string
    /// is, and the string ends a character, as it is one.
    pub fn as_str(&self) -> &str {
        match &self.0 {
            Form::Inline { len, bytes } => std::str::from_utf8(&bytes.0).ok().and_then(|s| s.get(..usize::from(*len))).unwrap_or_default(),
            Form::Shared(s) => s,
        }
    }

    /// Its length in octets.
    pub fn len(&self) -> usize {
        match &self.0 {
            Form::Inline { len, .. } => usize::from(*len),
            Form::Shared(s) => s.len(),
        }
    }

    /// Whether it has no characters.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether it is held in the value, with no buffer.
    pub fn is_inline(&self) -> bool {
        matches!(self.0, Form::Inline { .. })
    }

    /// Its octets without its trailing spaces [X3.135-1986 §5.11 GR5].
    pub fn unpadded(&self) -> &[u8] {
        let bytes = self.as_bytes();
        let kept = bytes.iter().rposition(|&b| b != SPACE).map_or(0, |last| last.saturating_add(1));
        bytes.get(..kept).unwrap_or_default()
    }

    /// Its characters, counted over its octets: those that begin one.
    pub fn char_count(&self) -> usize {
        self.as_bytes().iter().filter(|&&b| !continues(b)).count()
    }

    /// `text` followed by `pad` spaces [X3.135-1986 §8.7 GR7b]: in the
    /// value with no allocation if it fits, else built on the stack and
    /// copied to its buffer, the one allocation, or past `STACKED` octets
    /// built in a string first.
    pub fn padded(text: &str, pad: usize) -> CharString {
        if pad == 0 {
            return CharString::from(text);
        }
        let total = text.len().saturating_add(pad);
        let mut stack = [SPACE; STACKED];
        let Some(head) = stack.get_mut(..text.len()) else { return CharString::from(spaced(text, pad)) };
        head.copy_from_slice(text.as_bytes());
        let Some(whole) = stack.get(..total) else { return CharString::from(spaced(text, pad)) };
        if let Some(form) = held(whole) {
            return CharString(form);
        }
        std::str::from_utf8(whole).map_or_else(|_| CharString::from(spaced(text, pad)), |s| CharString(Form::Shared(Arc::from(s))))
    }

    /// `self` then `other`, in the value if both fit.
    #[must_use]
    pub fn joined(&self, other: &str) -> CharString {
        let total = self.len().saturating_add(other.len());
        let mut bytes = [0_u8; INLINE];
        if let Some(whole) = bytes.get_mut(..total)
            && let Some((head, tail)) = whole.split_at_mut_checked(self.len())
        {
            head.copy_from_slice(self.as_bytes());
            tail.copy_from_slice(other.as_bytes());
            return held(bytes.get(..total).unwrap_or_default()).map_or_else(CharString::new, CharString);
        }
        let mut text = String::with_capacity(total);
        text.push_str(self.as_str());
        text.push_str(other);
        CharString::from(text)
    }

    /// Its text as a `String`, copied.
    pub fn into_string(self) -> String {
        self.as_str().to_owned()
    }
}

/// Whether `octet` continues a character of UTF-8 rather than beginning
/// one.
const fn continues(octet: u8) -> bool {
    octet & CONTINUATION_MASK == CONTINUATION
}

/// The two high bits of an octet of UTF-8, which say whether it continues
/// a character.
const CONTINUATION_MASK: u8 = 0xC0;
/// Those bits of an octet that continues one.
const CONTINUATION: u8 = 0x80;

/// `text` followed by `pad` spaces, in a string: one allocation, nothing
/// formatted.
fn spaced(text: &str, pad: usize) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(pad));
    out.push_str(text);
    out.extend(std::iter::repeat_n(' ', pad));
    out
}

impl Default for CharString {
    fn default() -> CharString {
        CharString::new()
    }
}

impl Deref for CharString {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for CharString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for CharString {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

/// As a `str` shows itself, whatever the form.
impl fmt::Debug for CharString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for CharString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

/// Equal text is equal whatever the form: by its octets.
impl PartialEq for CharString {
    fn eq(&self, other: &CharString) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for CharString {}

/// The order of the text, which is that of its octets in UTF-8.
impl Ord for CharString {
    fn cmp(&self, other: &CharString) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl PartialOrd for CharString {
    fn partial_cmp(&self, other: &CharString) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// As its text hashes, so that a table keyed by one is asked by a `str`
/// (`Borrow`) and equal text hashes equal whatever the form.
impl Hash for CharString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl From<&str> for CharString {
    fn from(text: &str) -> CharString {
        CharString(held(text.as_bytes()).unwrap_or_else(|| Form::Shared(Arc::from(text))))
    }
}

impl From<String> for CharString {
    fn from(text: String) -> CharString {
        CharString(held(text.as_bytes()).unwrap_or_else(|| Form::Shared(Arc::from(text))))
    }
}

impl From<&String> for CharString {
    fn from(text: &String) -> CharString {
        CharString::from(text.as_str())
    }
}

impl From<Box<str>> for CharString {
    fn from(text: Box<str>) -> CharString {
        CharString(held(text.as_bytes()).unwrap_or_else(|| Form::Shared(Arc::from(text))))
    }
}

impl From<Arc<str>> for CharString {
    fn from(text: Arc<str>) -> CharString {
        CharString(held(text.as_bytes()).unwrap_or(Form::Shared(text)))
    }
}

impl From<Cow<'_, str>> for CharString {
    fn from(text: Cow<'_, str>) -> CharString {
        match text {
            Cow::Borrowed(s) => CharString::from(s),
            Cow::Owned(s) => CharString::from(s),
        }
    }
}

impl From<char> for CharString {
    fn from(c: char) -> CharString {
        let mut octets = [0; CHAR_OCTETS];
        CharString::from(&*c.encode_utf8(&mut octets))
    }
}

/// The octets a character takes at most in UTF-8.
const CHAR_OCTETS: usize = 4;

impl From<CharString> for String {
    fn from(text: CharString) -> String {
        text.into_string()
    }
}

impl From<&CharString> for String {
    fn from(text: &CharString) -> String {
        text.as_str().to_owned()
    }
}

impl FromIterator<char> for CharString {
    fn from_iter<I: IntoIterator<Item = char>>(chars: I) -> CharString {
        CharString::from(chars.into_iter().collect::<String>())
    }
}

impl<'a> FromIterator<&'a char> for CharString {
    fn from_iter<I: IntoIterator<Item = &'a char>>(chars: I) -> CharString {
        CharString::from(chars.into_iter().collect::<String>())
    }
}

impl<'a> FromIterator<&'a str> for CharString {
    fn from_iter<I: IntoIterator<Item = &'a str>>(parts: I) -> CharString {
        CharString::from(parts.into_iter().collect::<String>())
    }
}

impl FromIterator<String> for CharString {
    fn from_iter<I: IntoIterator<Item = String>>(parts: I) -> CharString {
        CharString::from(parts.into_iter().collect::<String>())
    }
}

impl PartialEq<str> for CharString {
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<&str> for CharString {
    fn eq(&self, other: &&str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<String> for CharString {
    fn eq(&self, other: &String) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<CharString> for str {
    fn eq(&self, other: &CharString) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<CharString> for &str {
    fn eq(&self, other: &CharString) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<CharString> for String {
    fn eq(&self, other: &CharString) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

#[cfg(test)]
mod tests;
