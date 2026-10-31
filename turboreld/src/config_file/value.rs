//! \[DESIGN\] The values of a configuration file (`config_file.rs` gives
//! their grammar): strings, integers, floats, booleans and arrays of
//! strings, each the value of a setting. Split from config_file.rs
//! (RULES.md R-SIZE-2).

// The words of a switch, which `true` and `false` give (RULES.md D36).
use turborel::settings::{OFF, ON};
use turborel::value::{Decimal, Value};

use super::{At, Reader, Refusal, character, refuse};

/// TOML's word of the boolean true.
const TRUE: &str = "true";

/// TOML's word of the boolean false.
const FALSE: &str = "false";

/// TOML's word of an infinite float, which the subset refuses.
const INF: &str = "inf";

/// TOML's word of a float not a number, which the subset refuses.
const NAN: &str = "nan";

/// The prefixes of TOML's integers other than decimal, which the subset
/// refuses.
const OTHER_BASES: [&str; 3] = ["0x", "0o", "0b"];

/// What joins an array's strings into the value of a list setting.
const SEPARATOR: &str = ",";

/// The hexadecimal digits of a `\u` escape (TOML 1.0).
const SHORT_ESCAPE_DIGITS: usize = 4;

/// The hexadecimal digits of a `\U` escape (TOML 1.0).
const LONG_ESCAPE_DIGITS: usize = 8;

/// The radix of an escape's digits.
const HEX: u32 = 16;

/// What a value is told to be, where none is.
const EXPECTED: &str = "a value is expected: a quoted string, an integer, a float, true, false, or an array of strings";

/// What a number is, by its form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// An `<integer>`.
    Integer,
    /// A `<float>`.
    Float,
}

impl Reader {
    /// A `<value>`, as the value of a setting.
    pub(super) fn value(&mut self) -> Result<Value, Refusal> {
        let at = self.at;
        match self.peek() {
            Some('"') => self.basic_string().map(Value::Char),
            Some('\'') => self.literal_string().map(Value::Char),
            Some('[') => self.array(),
            Some('{') => refuse(at, "inline tables, {...}, are not taken"),
            Some('+' | '-' | '0'..='9') => self.number(),
            Some(c) => {
                if c.is_ascii_alphabetic() {
                    self.word()
                } else {
                    refuse(at, EXPECTED)
                }
            }
            None => refuse(at, EXPECTED),
        }
    }

    /// A `<boolean>`, as the word of a switch.
    fn word(&mut self) -> Result<Value, Refusal> {
        let at = self.at;
        let mut word = String::new();
        while let Some(c) = self.peek() {
            if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
                break;
            }
            word.push(c);
            self.bump();
        }
        match word.as_str() {
            TRUE => Ok(Value::Char(ON.to_owned())),
            FALSE => Ok(Value::Char(OFF.to_owned())),
            INF | NAN => refuse(at, "infinite and not-a-number floats are not taken"),
            other => refuse(at, &format!("{other} is not a value: a string is quoted, \"{other}\"; a boolean is true or false")),
        }
    }

    /// An `<integer>` or a `<float>`.
    fn number(&mut self) -> Result<Value, Refusal> {
        let at = self.at;
        let mut token = String::new();
        while let Some(c) = self.peek() {
            if !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '_' | ':')) {
                break;
            }
            token.push(c);
            self.bump();
        }
        number(&token).map_err(|message| Refusal { at, message })
    }

    /// A `<basic string>`, on one line, its escapes read.
    pub(super) fn basic_string(&mut self) -> Result<String, Refusal> {
        let at = self.at;
        self.single_line('"')?;
        let mut text = String::new();
        loop {
            let here = self.at;
            match self.bump() {
                None | Some('\n') => return refuse(at, "the string is not closed on its line"),
                Some('"') => return Ok(text),
                Some('\\') => text.push(self.escape(here)?),
                Some(c) => text.push(character(c, here)?),
            }
        }
    }

    /// A `<literal string>`, on one line, as written.
    pub(super) fn literal_string(&mut self) -> Result<String, Refusal> {
        let at = self.at;
        self.single_line('\'')?;
        let mut text = String::new();
        loop {
            let here = self.at;
            match self.bump() {
                None | Some('\n') => return refuse(at, "the string is not closed on its line"),
                Some('\'') => return Ok(text),
                Some(c) => text.push(character(c, here)?),
            }
        }
    }

    /// The opening `quote` of a string, taken, unless it opens a
    /// multi-line string, three of it, which the subset refuses.
    fn single_line(&mut self, quote: char) -> Result<(), Refusal> {
        if self.peek_at(1) == Some(quote) && self.peek_at(2) == Some(quote) {
            return refuse(self.at, "multi-line strings, \"\"\" or ''', are not taken");
        }
        self.bump();
        Ok(())
    }

    /// The character an `<escape>` stands for, after its reverse solidus,
    /// which is `at`.
    fn escape(&mut self, at: At) -> Result<char, Refusal> {
        match self.bump() {
            Some('b') => Ok('\u{8}'),
            Some('t') => Ok('\t'),
            Some('n') => Ok('\n'),
            Some('f') => Ok('\u{c}'),
            Some('r') => Ok('\r'),
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('u') => self.unicode(at, SHORT_ESCAPE_DIGITS),
            Some('U') => self.unicode(at, LONG_ESCAPE_DIGITS),
            Some(c) => refuse(at, &format!("\\{c} is not an escape: \\b, \\t, \\n, \\f, \\r, \\\", \\\\, \\uXXXX and \\UXXXXXXXX are")),
            None => refuse(at, "the string is not closed on its line"),
        }
    }

    /// The Unicode scalar value of `digits` hexadecimal digits, of an
    /// escape `at`.
    fn unicode(&mut self, at: At, digits: usize) -> Result<char, Refusal> {
        let mut code: u32 = 0;
        for _ in 0..digits {
            let Some(digit) = self.peek().and_then(|c| c.to_digit(HEX)) else {
                return refuse(at, "an escape \\u takes 4 hexadecimal digits, and \\U 8");
            };
            self.bump();
            code = code.checked_mul(HEX).and_then(|c| c.checked_add(digit)).unwrap_or(u32::MAX);
        }
        char::from_u32(code).ok_or_else(|| Refusal { at, message: format!("U+{code:X} is not a Unicode scalar value") })
    }

    /// An `<array>` of strings, as their value: joined by commas.
    fn array(&mut self) -> Result<Value, Refusal> {
        let at = self.at;
        self.bump();
        let mut strings = Vec::new();
        loop {
            self.array_blank()?;
            let here = self.at;
            match self.peek() {
                Some(']') => break,
                Some('"' | '\'') => strings.push(self.element()?),
                Some(_) => return refuse(here, "an array holds strings only, by commas, and ends with ]"),
                None => return refuse(at, "the array is not closed"),
            }
            self.array_blank()?;
            let after = self.at;
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => break,
                Some(_) => return refuse(after, "an array's strings are separated by commas, and it ends with ]"),
                None => return refuse(at, "the array is not closed"),
            }
        }
        self.bump();
        Ok(Value::Char(strings.join(SEPARATOR)))
    }

    /// A string of an array, holding no comma, as the strings are given
    /// to their setting joined by commas.
    fn element(&mut self) -> Result<String, Refusal> {
        let at = self.at;
        let string = if self.peek() == Some('"') { self.basic_string()? } else { self.literal_string()? };
        if string.contains(SEPARATOR) {
            return refuse(at, "a string of an array holds no comma: the strings are given to the setting joined by commas");
        }
        Ok(string)
    }

    /// An `<array blank>`: spaces, tabs, newlines and comments.
    fn array_blank(&mut self) -> Result<(), Refusal> {
        loop {
            match self.peek() {
                Some(' ' | '\t' | '\n') => {
                    self.bump();
                }
                Some('\r') => {
                    self.bump();
                    self.expect('\n', "a carriage return is followed by a line feed")?;
                }
                Some('#') => self.comment()?,
                Some(_) | None => return Ok(()),
            }
        }
    }
}

/// The value of the number `token`: an `<integer>`, an exact whole number,
/// or a `<float>`, an approximate one.
fn number(token: &str) -> Result<Value, String> {
    let unsigned = token.strip_prefix(['+', '-']).unwrap_or(token);
    if is_date(unsigned) {
        return Err("dates and times are not taken".to_owned());
    }
    if matches!(unsigned, INF | NAN) {
        return Err("infinite and not-a-number floats are not taken".to_owned());
    }
    if OTHER_BASES.iter().any(|prefix| unsigned.starts_with(prefix)) {
        return Err("an integer is decimal: 0x, 0o and 0b are not taken".to_owned());
    }
    let shape =
        shape(unsigned).ok_or_else(|| format!("{token} is not a number: its digits are separated by single underscores, if any"))?;
    let digits: String = token.chars().filter(|c| *c != '_').collect();
    match shape {
        Shape::Integer => {
            let whole = digits.parse::<i64>().map_err(|_| format!("{token} is past the integers taken, of 64 bits"))?;
            Ok(Value::Exact(Decimal::new(i128::from(whole), 0)))
        }
        Shape::Float => {
            let x = digits.parse::<f64>().ok().filter(|x| x.is_finite());
            x.map(Value::Approx).ok_or_else(|| format!("{token} is past the floats taken"))
        }
    }
}

/// Whether `token`, without its sign, is a date or a time, as TOML writes
/// them: with a colon, or a minus sign but in an exponent.
fn is_date(token: &str) -> bool {
    let minus = token.chars().zip(token.chars().skip(1)).any(|(before, c)| c == '-' && !matches!(before, 'e' | 'E'));
    token.contains(':') || minus
}

/// What the number `token`, without its sign, is by its form; none if it
/// is neither an `<integer>` nor a `<float>`.
fn shape(token: &str) -> Option<Shape> {
    let chars: Vec<char> = token.chars().collect();
    let mut rest = digits(&chars, false)?;
    let mut shape = Shape::Integer;
    if let Some(('.', after)) = split(rest) {
        rest = digits(after, true)?;
        shape = Shape::Float;
    }
    if let Some(('e' | 'E', after)) = split(rest) {
        let unsigned = match split(after) {
            Some(('+' | '-', exponent)) => exponent,
            Some(_) | None => after,
        };
        rest = digits(unsigned, true)?;
        shape = Shape::Float;
    }
    rest.is_empty().then_some(shape)
}

/// The first of `chars` and the rest, if there is one.
fn split(chars: &[char]) -> Option<(char, &[char])> {
    chars.split_first().map(|(c, rest)| (*c, rest))
}

/// What follows `<digits>` at the start of `chars`, a 0 leading more of
/// them only where `leading_zeros`; none if they are not there.
fn digits(chars: &[char], leading_zeros: bool) -> Option<&[char]> {
    let run = chars.iter().take_while(|c| c.is_ascii_digit() || **c == '_').count();
    let (body, rest) = chars.split_at_checked(run)?;
    let (first, last) = (*body.first()?, *body.last()?);
    let doubled = body.windows(2).any(|pair| pair == ['_', '_']);
    let leading = !leading_zeros && first == '0' && body.len() > 1;
    (first != '_' && last != '_' && !doubled && !leading).then_some(rest)
}
