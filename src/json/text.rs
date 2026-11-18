//! JSON text parsed to an SQL/JSON item [ISO/IEC 9075-2:2023 §9.42], by
//! the grammar of RFC 8259 [RFC 8259 §2 to §7]: `JSON-text`, `value`,
//! `object`, `member`, `array`, `number` and `string`.
//!
//! [§9.42 GR3a iii 3 A IV] A JSON number is "the value of the `<signed
//! numeric literal>` whose characters are identical" to it: exact without
//! an exponent, approximate with one. \[DESIGN\] An exact one of more
//! significant digits than an exact number holds is a decimal
//! floating-point number, which holds it exactly; its leading zeros, and
//! the trailing zeros of its fraction, are not counted. RFC 8259 §6 lets
//! an implementation set the range and precision of the numbers it
//! accepts, and [§5.3 SR29] has the value of a literal held exactly, so a
//! number no number of turborel's holds exactly is not rounded but is
//! SQLSTATE 22003, numeric value out of range, as one past an approximate
//! number's range is, too great or, not zero, too small.

use std::collections::HashSet;
use std::num::FpCategory;
use std::ops::RangeInclusive;

use super::{Item, MAX_NESTING, Member, Scalar, beyond};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::{DecFloat, Decimal, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Value};

// [RFC 8259 §3] The literal names.
/// `false`.
const FALSE: &str = "false";
/// `null`.
const NULL: &str = "null";
/// `true`.
const TRUE: &str = "true";

// [RFC 8259 §7] Strings and their escapes.
/// The last of "the control characters (U+0000 through U+001F)", which a
/// string holds only escaped.
const LAST_CONTROL: char = '\u{1F}';
/// The base of the "four hexadecimal digits" of a `\u` escape.
pub(super) const HEX: u32 = 16;
/// How many hexadecimal digits a `\u` escape has.
const HEX_DIGITS: u32 = 4;
/// The first code units of "the UTF-16 surrogate pair" of a character
/// outside the Basic Multilingual Plane.
pub(super) const HIGH_SURROGATES: RangeInclusive<u32> = 0xD800..=0xDBFF;
/// The second code units of a surrogate pair.
const LOW_SURROGATES: RangeInclusive<u32> = 0xDC00..=0xDFFF;
/// How many bits of a character each code unit of a surrogate pair holds.
const SURROGATE_BITS: u32 = 10;
/// The first code point outside the Basic Multilingual Plane, which a
/// surrogate pair counts from.
const SUPPLEMENTARY: u32 = 0x1_0000;

/// [§9.42] The UNIQUENESS CONSTRAINT a JSON text is parsed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keys {
    /// WITHOUT UNIQUE KEYS: two members of an object may have one key.
    Any,
    /// WITH UNIQUE KEYS: no two members of an object have one key.
    Unique,
}

/// A JSON text being read.
struct Reader {
    chars: Vec<char>,
    at: usize,
    keys: Keys,
}

/// [§9.42 GR3a] The SQL/JSON item the JSON text `text` is, its objects'
/// keys as `keys` says. \[GR3a iii 3 A VIII 2\] Members of one key are
/// all kept, in the order written.
///
/// # Errors
///
/// SQLSTATE 22032 if `text` is no JSON text \[GR3a iii 1\], or a string of
/// it escapes a surrogate that is not of a pair, no Unicode character;
/// SQLSTATE 22037 if two members of an object have one key under
/// `Keys::Unique` \[GR3a iii 2\]; SQLSTATE 22003 if a number is one no
/// number of turborel's holds exactly (`exact`, `approximate`); SQLCODE
/// -900 if arrays and objects nest deeper than `MAX_NESTING`.
pub fn parse(text: &str, keys: Keys) -> Result<Item> {
    let mut reader = Reader { chars: text.chars().collect(), at: 0, keys };
    reader.ws();
    let item = reader.value(0)?;
    reader.ws();
    if reader.peek().is_some() {
        return Err(reader.invalid("characters follow its value"));
    }
    Ok(item)
}

/// The error of two members of the key `key` in an object that is to have
/// unique keys.
fn non_unique(key: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.42 GR3a iii 2] two members of a JSON object have the key \"{key}\""))
        .with_state(SqlState::NON_UNIQUE_KEYS_IN_A_JSON_OBJECT)
}

/// The exact number of the sign `negative` says, the digits `whole`
/// before its decimal point and `fraction` after it, either of them empty
/// if the other is not. Its leading zeros, and the trailing zeros of its
/// fraction, are insignificant: of more digits than `MAX_EXACT_PRECISION`,
/// it is exact without them if they are what made it too long, else the
/// decimal floating-point number it is, which holds
/// `MAX_DECFLOAT_PRECISION` significant digits exactly.
///
/// # Errors
///
/// SQLSTATE 22003 if it has more significant digits than a decimal
/// floating-point number holds, since no number of turborel's is the
/// value of its `<signed numeric literal>` [§5.3 SR29], or is out of a
/// decimal floating-point number's range.
pub(super) fn exact(negative: bool, whole: &str, fraction: &str) -> Result<Item> {
    let digits = format!("{whole}{fraction}");
    let significant = fraction.trim_end_matches('0');
    let trimmed = format!("{whole}{significant}");
    let trimmed = trimmed.trim_start_matches('0');
    let (units, scale) = if fits(&digits, fraction) { (digits.as_str(), fraction.len()) } else { (trimmed, significant.len()) };
    if fits(units, "") && u32::try_from(scale).is_ok_and(|scale| scale <= MAX_EXACT_PRECISION) {
        // At most MAX_EXACT_PRECISION digits always fit an i128 and a u32,
        // so neither fallback below is ever taken.
        let units: i128 = if units.is_empty() { 0 } else { units.parse().unwrap_or(0) };
        let scale = u32::try_from(scale).unwrap_or(MAX_EXACT_PRECISION);
        return Ok(Item::Scalar(Decimal::new(if negative { units.wrapping_neg() } else { units }, scale).into()));
    }
    let coefficient = trimmed.trim_end_matches('0');
    if u32::try_from(coefficient.len()).is_ok_and(|count| count > MAX_DECFLOAT_PRECISION) {
        return Err(out_of_range(&format!("the number has more than {MAX_DECFLOAT_PRECISION} significant digits")));
    }
    let exponent = i64::try_from(fraction.len()).unwrap_or(i64::MAX).saturating_neg();
    DecFloat::of_digits(negative, &digits, exponent, MAX_DECFLOAT_PRECISION)
        .map(|number| Item::Scalar(number.into()))
        .ok_or_else(|| out_of_range("the number is out of the range of a decimal floating-point number"))
}

/// Whether `digits`, of which `fraction` are after the decimal point, are
/// few enough for an exact number.
fn fits(digits: &str, fraction: &str) -> bool {
    u32::try_from(digits.len()).is_ok_and(|count| count <= MAX_EXACT_PRECISION)
        && u32::try_from(fraction.len()).is_ok_and(|count| count <= MAX_EXACT_PRECISION)
}

/// The approximate number the number `text`, one with an exponent, is:
/// a JSON number, or an SQL/JSON path language numeric literal.
///
/// # Errors
///
/// SQLSTATE 22003 if it is out of the range of an approximate number, too
/// great in magnitude or, not zero, too small to be other than zero.
pub(super) fn approximate(text: &str) -> Result<Item> {
    let number = text.parse::<f64>().ok().filter(|number| number.is_finite());
    let Some(number) = number else { return Err(out_of_range("the number is too great for an approximate number")) };
    let mantissa = text.split(['e', 'E']).next().unwrap_or_default();
    if number.classify() == FpCategory::Zero && mantissa.chars().any(|c| matches!(c, '1'..='9')) {
        return Err(out_of_range("the number is too small for an approximate number, but not zero"));
    }
    Scalar::new(Value::Approx(number)).map(Item::Scalar).ok_or_else(|| out_of_range("the number is not an approximate number"))
}

/// [§9.42 GR3a iii 3 A IV] The exception of a number no number of
/// turborel's is the value of, `why`: numeric value out of range.
fn out_of_range(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A IV] {why}")).with_state(SqlState::NUMERIC_VALUE_OUT_OF_RANGE)
}

/// The character of "the UTF-16 surrogate pair" of the code units `high`,
/// a high surrogate, and `low`; none if `low` is no low surrogate.
pub(super) fn paired(high: u32, low: u32) -> Option<char> {
    if !LOW_SURROGATES.contains(&low) {
        return None;
    }
    let above = high.wrapping_sub(*HIGH_SURROGATES.start()).wrapping_shl(SURROGATE_BITS);
    char::from_u32(SUPPLEMENTARY.wrapping_add(above).wrapping_add(low.wrapping_sub(*LOW_SURROGATES.start())))
}

impl Reader {
    /// The character next, if one is.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    /// Past the character next.
    fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    /// Whether `c` is next, taken if it is.
    fn accept(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.bump();
        }
        found
    }

    /// [RFC 8259 §2] Past `ws`, "insignificant whitespace".
    fn ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.bump();
        }
    }

    /// The error of a string that is no JSON text, `why`, where the
    /// reader is.
    fn invalid(&self, why: &str) -> SqlError {
        let character = self.at.saturating_add(1);
        SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-2:2023 §9.42 GR3a iii 1] the string is not a JSON text: {why} (at character {character})"),
        )
        .with_state(SqlState::INVALID_JSON_TEXT)
    }

    /// [RFC 8259 §3] `value`, within `depth` arrays and objects.
    fn value(&mut self, depth: usize) -> Result<Item> {
        match self.peek() {
            Some('{') => self.object(depth.saturating_add(1)),
            Some('[') => self.array(depth.saturating_add(1)),
            Some('"') => Ok(Item::Scalar(self.string()?.into())),
            Some('-' | '0'..='9') => self.number(),
            Some('f') => self.name(FALSE, Item::Scalar(false.into())),
            Some('n') => self.name(NULL, Item::Null),
            Some('t') => self.name(TRUE, Item::Scalar(true.into())),
            Some(_) => Err(self.invalid("no value begins so")),
            None => Err(self.invalid("a value is missing")),
        }
    }

    /// [RFC 8259 §3] The literal name `word`, which is `item`.
    fn name(&mut self, word: &str, item: Item) -> Result<Item> {
        for c in word.chars() {
            if !self.accept(c) {
                return Err(self.invalid("a literal name is none of false, null and true"));
            }
        }
        Ok(item)
    }

    /// Into an array or an object, the `depth`th around what follows, past
    /// the character that begins it.
    fn enter(&mut self, depth: usize) -> Result<()> {
        if depth > MAX_NESTING {
            return Err(beyond("arrays and objects nest too deep"));
        }
        self.bump();
        self.ws();
        Ok(())
    }

    /// [RFC 8259 §5] `array`.
    fn array(&mut self, depth: usize) -> Result<Item> {
        self.enter(depth)?;
        let mut elements = Vec::new();
        if self.accept(']') {
            return Ok(Item::Array(elements));
        }
        loop {
            elements.push(self.value(depth)?);
            self.ws();
            if self.accept(']') {
                return Ok(Item::Array(elements));
            }
            if !self.accept(',') {
                return Err(self.invalid("a ',' or a ']' follows an array's element"));
            }
            self.ws();
        }
    }

    /// [RFC 8259 §4] `object`.
    fn object(&mut self, depth: usize) -> Result<Item> {
        self.enter(depth)?;
        let mut members = Vec::new();
        if self.accept('}') {
            return Ok(Item::Object(members));
        }
        loop {
            members.push(self.member(depth)?);
            self.ws();
            if self.accept('}') {
                break;
            }
            if !self.accept(',') {
                return Err(self.invalid("a ',' or a '}' follows an object's member"));
            }
            self.ws();
        }
        self.unique(&members)?;
        Ok(Item::Object(members))
    }

    /// [RFC 8259 §4] `member`.
    fn member(&mut self, depth: usize) -> Result<Member> {
        if self.peek() != Some('"') {
            return Err(self.invalid("a member begins with its name, a string"));
        }
        let key = self.string()?;
        self.ws();
        if !self.accept(':') {
            return Err(self.invalid("a ':' follows a member's name"));
        }
        self.ws();
        Ok(Member { key, value: self.value(depth)? })
    }

    /// [§9.42 GR3a iii 2] That no two of `members` have one key, if keys
    /// are to be unique.
    fn unique(&self, members: &[Member]) -> Result<()> {
        if self.keys == Keys::Any {
            return Ok(());
        }
        let mut seen = HashSet::new();
        for member in members {
            if !seen.insert(member.key.as_str()) {
                return Err(non_unique(&member.key));
            }
        }
        Ok(())
    }

    /// [RFC 8259 §7] `string`: its characters, each escape replaced by
    /// what it stands for [§9.42 GR3a iii 3 A V].
    fn string(&mut self) -> Result<String> {
        self.bump();
        let mut string = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.invalid("a string is not closed"));
            };
            match c {
                '"' => {
                    self.bump();
                    return Ok(string);
                }
                '\\' => {
                    self.bump();
                    string.push(self.escaped()?);
                }
                '\0'..=LAST_CONTROL => return Err(self.invalid("a control character in a string is not escaped")),
                _ => {
                    self.bump();
                    string.push(c);
                }
            }
        }
    }

    /// [RFC 8259 §7] The character the escape whose reverse solidus is
    /// read stands for.
    fn escaped(&mut self) -> Result<char> {
        let Some(c) = self.peek() else {
            return Err(self.invalid("a string is not closed"));
        };
        let character = match c {
            '"' | '\\' | '/' => c,
            'b' => '\u{8}',
            'f' => '\u{C}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => {
                self.bump();
                return self.code_point();
            }
            _ => return Err(self.invalid("an escape is none of \\\" \\\\ \\/ \\b \\f \\n \\r \\t and \\u")),
        };
        self.bump();
        Ok(character)
    }

    /// [RFC 8259 §7] The character a `\u` escape whose `u` is read stands
    /// for: of its four hexadecimal digits, or of them and the next
    /// escape's, "the UTF-16 surrogate pair". \[DESIGN\] A surrogate not
    /// of a pair is no Unicode character, so no character of an SQL
    /// character string [§9.42 GR3a iii 3 A V], and is refused.
    fn code_point(&mut self) -> Result<char> {
        let first = self.hex()?;
        if !HIGH_SURROGATES.contains(&first) {
            return char::from_u32(first).ok_or_else(|| self.invalid("a low surrogate follows no high surrogate"));
        }
        let second = self.second_escape()?;
        paired(first, second).ok_or_else(|| self.invalid("no low surrogate follows a high surrogate"))
    }

    /// The value of the `\u` escape next, taken; 0, no surrogate, if none
    /// is next.
    fn second_escape(&mut self) -> Result<u32> {
        if !self.accept('\\') {
            return Ok(0);
        }
        if !self.accept('u') {
            return Ok(0);
        }
        self.hex()
    }

    /// The value of a `\u` escape's four hexadecimal digits.
    fn hex(&mut self) -> Result<u32> {
        let mut value: u32 = 0;
        for _ in 0..HEX_DIGITS {
            let Some(digit) = self.peek().and_then(|c| c.to_digit(HEX)) else {
                return Err(self.invalid("four hexadecimal digits follow \\u"));
            };
            self.bump();
            value = value.wrapping_mul(HEX).wrapping_add(digit);
        }
        Ok(value)
    }

    /// [RFC 8259 §6] `number`.
    fn number(&mut self) -> Result<Item> {
        let start = self.at;
        let negative = self.accept('-');
        let whole = if self.accept('0') { String::from('0') } else { self.digits("a number's digits are missing")? };
        let fraction = if self.accept('.') { self.digits("a digit follows a number's '.'")? } else { String::new() };
        if !matches!(self.peek(), Some('e' | 'E')) {
            return exact(negative, &whole, &fraction);
        }
        self.bump();
        if !self.accept('+') {
            let _minus = self.accept('-');
        }
        self.digits("a digit follows a number's exponent")?;
        let text: String = self.chars.get(start..self.at).map_or_else(String::new, |read| read.iter().collect());
        approximate(&text)
    }

    /// [RFC 8259 §6] `1*DIGIT`: the digits next, one at least, else the
    /// error `why`.
    fn digits(&mut self, why: &str) -> Result<String> {
        let mut digits = String::new();
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            digits.push(c);
            self.bump();
        }
        if digits.is_empty() { Err(self.invalid(why)) } else { Ok(digits) }
    }
}

#[cfg(test)]
mod tests;
