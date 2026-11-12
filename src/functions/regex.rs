//! [ISO/IEC 9075-2:2023 §8.7, §6.31, §6.33] What matches an XQuery regular
//! expression (`regex.rs`): the `<regex like predicate>`, as a function of
//! BOOLEAN that `Cond::Value` judges; OCCURRENCES_REGEX, POSITION_REGEX and
//! SUBSTRING_REGEX (`regex/search.rs`); and TRANSLATE_REGEX
//! (`regex/translate.rs`).
//!
//! The binder makes each call its own function, so each keeps the last
//! pattern it compiled, and none is registered. The parser passes what is
//! a key word in the syntax, the units and START, AFTER or ALL, as
//! character literals before the operands, as TRIM passes its
//! specification.

mod search;
mod translate;

use std::sync::{Arc, Mutex};

pub use search::{OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX};
pub use translate::TRANSLATE_REGEX;

use super::ScalarFunction;
use super::strings::{character, text};
use crate::error::{Code, Result, SqlError, fail};
use crate::params::ParameterType;
use crate::regex::Regex;
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

/// [ISO/IEC 9075-2:2023 §8.7] LIKE_REGEX, no key word of SQL-92.
pub const LIKE_REGEX: &str = "LIKE_REGEX";
/// [ISO/IEC 9075-2:2023 §8.7] FLAG, no key word of SQL-92, before an
/// `<XQuery option flag>`.
pub const FLAG: &str = "FLAG";
/// [ISO/IEC 9075-2:2023 §6.31] START, no key word of SQL-92: the position
/// where a match begins.
pub const START: &str = "START";
/// [ISO/IEC 9075-2:2023 §6.31] AFTER, no key word of SQL-92: the position
/// after a match ends.
pub const AFTER: &str = "AFTER";
/// [ISO/IEC 9075-2:2023 §6.31, §6.33] OCCURRENCE, no key word of SQL-92,
/// before a `<regex occurrence>`.
pub const OCCURRENCE: &str = "OCCURRENCE";
/// [ISO/IEC 9075-2:2023 §6.1] CHARACTERS, no key word of SQL-92, a `<char
/// length units>`.
pub const CHARACTERS: &str = "CHARACTERS";
/// [ISO/IEC 9075-2:2023 §6.1] OCTETS, no key word of SQL-92, a `<char
/// length units>`: octets of the string as UTF-8, its form of use.
pub const OCTETS: &str = "OCTETS";
/// The regular expression functions, each before "(" no identifier.
pub const REGEX_FUNCTIONS: [&str; 4] = [OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX, TRANSLATE_REGEX];

/// [§6.31 SR1] The type of a count or a position: exact numeric of the
/// greatest precision, scale 0 (IV137).
const COUNTED: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };

/// The regular expression function `name`, a new one; none if it is no
/// such function.
pub fn regex_function(name: &str) -> Option<Arc<dyn ScalarFunction>> {
    let f: Arc<dyn ScalarFunction> = match REGEX_FUNCTIONS.iter().position(|f| *f == name)? {
        0 => Arc::new(search::Occurrences::default()),
        1 => Arc::new(search::Position::default()),
        2 => Arc::new(search::Substring::default()),
        _ => Arc::new(translate::Translate::default()),
    };
    Some(f)
}

/// Of the regular expression function `name`, which argument is its
/// subject, whose collation is its result's [§6.33 SR8f ii, SR12f ii]; none
/// if it gives no string.
pub fn subject_of(name: &str) -> Option<usize> {
    if name == SUBSTRING_REGEX {
        Some(search::SUBSTRING_SUBJECT)
    } else if name == TRANSLATE_REGEX {
        Some(translate::SUBJECT)
    } else {
        None
    }
}

/// A pattern and a flag, and what they compile to.
type Compiled = (String, String, Arc<Regex>);

/// The last pattern a call compiled, so a constant one is compiled once,
/// and one from a column [Feature F847] each time it changes.
#[derive(Default)]
struct Cache {
    last: Mutex<Option<Compiled>>,
}

impl Cache {
    /// `pattern` compiled under `flag`, kept from the last call if it was
    /// the same.
    fn compiled(&self, pattern: &str, flag: &str) -> Result<Arc<Regex>> {
        let poisoned = |_| SqlError::new(Code::Api, "[DESIGN] a regular expression's compiled pattern was left poisoned by a panic");
        {
            let last = self.last.lock().map_err(poisoned)?;
            if let Some((.., regex)) = last.as_ref().filter(|(p, f, _)| p == pattern && f == flag) {
                return Ok(Arc::clone(regex));
            }
        }
        let regex = Arc::new(Regex::new(pattern, flag)?);
        *self.last.lock().map_err(poisoned)? = Some((pattern.to_owned(), flag.to_owned(), Arc::clone(&regex)));
        Ok(regex)
    }
}

/// [§6.31 SR3, SR4; §6.33 SR3] Of the function `name`, each of `strings`
/// a character string and each of `numbers` an exact number of scale 0.
fn operands(name: &str, strings: &[(&str, &DataType)], numbers: &[(&str, &DataType)]) -> Result<()> {
    for (what, t) in strings {
        if !t.is_character() {
            return fail(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.31 SR3a] the {what} of {name} is a character string, not {t}"));
        }
    }
    for (what, t) in numbers {
        if !matches!(t, DataType::Exact { scale: 0, .. }) {
            return fail(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §6.33 SR3] the {what} of {name} is an exact number of scale 0, not {t}"),
            );
        }
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §20.7 GR5a xxxvi, xxxvii] The type of a `?` at
/// `position` of a function whose `strings` are its pattern, flag, subject
/// and replacement and whose `numbers` its start position, occurrence and
/// group: CHARACTER VARYING beside another of the strings not a `?`;
/// NUMERIC(MP) of a number.
fn parameter(position: usize, args: &[Option<DataType>], strings: &[usize], numbers: &[usize]) -> Option<ParameterType> {
    if numbers.contains(&position) {
        return Some(ParameterType::Of(COUNTED));
    }
    let typed = strings.iter().any(|i| *i != position && args.get(*i).is_some_and(Option::is_some));
    (strings.contains(&position) && typed).then_some(ParameterType::Varying)
}

/// `n`, a count or position.
fn counted(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A subject string, and the units its positions and lengths count
/// [§9.28 GR11b].
struct Subject {
    chars: Vec<char>,
    octets: bool,
}

impl Subject {
    /// `s` counted in `units`, a `<char length units>`.
    fn new(s: &str, units: &Value) -> Result<Subject> {
        let octets = match text(units)? {
            Some(OCTETS) => true,
            Some(CHARACTERS) => false,
            Some(_) | None => return fail(Code::Api, "[DESIGN] a regular expression function was given units that are none"),
        };
        Ok(Subject { chars: s.chars().collect(), octets })
    }

    /// The units of characters `from` to `to`.
    fn units(&self, from: usize, to: usize) -> usize {
        let part = self.chars.get(from..to).unwrap_or_default();
        if self.octets { part.iter().map(|c| c.len_utf8()).sum() } else { part.len() }
    }

    /// [§6.31 GR18b ii] CHARACTER_LENGTH ( RSS USING CLU ).
    fn length(&self) -> usize {
        self.units(0, self.chars.len())
    }

    /// The position, from 1, of character `at`.
    fn position(&self, at: usize) -> i128 {
        i128::try_from(self.units(0, at)).unwrap_or(i128::MAX).saturating_add(1)
    }

    /// The character `sp`, a start position, names: none if it is less
    /// than 1 or past the string's length [§6.31 GR18b ii]. An octet
    /// within a character names the next character (UV074).
    fn start(&self, sp: i128) -> Option<usize> {
        let length = i128::try_from(self.length()).ok()?;
        if sp < 1 || sp > length {
            return None;
        }
        let offset = usize::try_from(sp.checked_sub(1)?).ok()?;
        (0..=self.chars.len()).find(|at| self.units(0, *at) >= offset)
    }
}

/// CVE LIKE_REGEX PAT FLAG FL, called with CVE, PAT and FL.
#[derive(Default)]
pub struct LikeRegex {
    cache: Cache,
}

impl ScalarFunction for LikeRegex {
    fn name(&self) -> &str {
        LIKE_REGEX
    }
    /// \[SR1\] Of a character string; its pattern and flag character
    /// strings, being `<character value expression>`s; BOOLEAN.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [value, pattern, flag] = args else { return fail(Code::Api, "[DESIGN] LIKE_REGEX takes a value, a pattern and a flag") };
        for (what, t) in [("value", value), ("pattern", pattern), ("flag", flag)] {
            if !t.is_character() {
                return fail(
                    Code::SyntaxRule,
                    format!("[ISO/IEC 9075-2:2023 §8.7 SR1] the {what} of LIKE_REGEX is a character string, not {t}"),
                );
            }
        }
        character(LIKE_REGEX, value).map(|_| DataType::Boolean)
    }
    /// [ISO/IEC 9075-2:2023 §20.7 GR5a xxxvi] A pattern or flag CHARACTER
    /// VARYING beside the other not a `?`, the implicit flag counting; the
    /// value none, being no `<regex subject string>`.
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position, args, &[1, 2], &[])
    }
    /// \[GR1\] Unknown if any operand is null; else whether the value
    /// matches [§9.28].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [value, pattern, flag] = args else { return fail(Code::Api, "[DESIGN] LIKE_REGEX takes a value, a pattern and a flag") };
        let (Some(value), Some(pattern), Some(flag)) = (text(value)?, text(pattern)?, text(flag)?) else { return Ok(Value::Null) };
        Ok(Value::Bool(self.cache.compiled(pattern, flag)?.is_match(value)?))
    }
}
