//! [ISO/IEC 9075-2:2023 §8.7, §6.31, §6.33] What matches an XQuery regular
//! expression (`regex.rs`): the `<regex like predicate>`, as a function of
//! BOOLEAN that `Cond::Value` judges; OCCURRENCES_REGEX, POSITION_REGEX and
//! SUBSTRING_REGEX (`regex/search.rs`); and TRANSLATE_REGEX
//! (`regex/translate.rs`).
//!
//! A subject may be a character large object, read a chunk at a time and
//! never held whole (RULES.md D79); SUBSTRING_REGEX and TRANSLATE_REGEX of
//! one give one [§6.33 SR8f i 2, SR12f i].
//!
//! The binder makes each call its own function, so each keeps the last
//! pattern it compiled, and none is registered. The parser passes what is
//! a key word in the syntax, the units and START, AFTER or ALL, as
//! character literals before the operands, as TRIM passes its
//! specification.

mod search;
mod subject;
mod translate;

use std::sync::{Arc, Mutex};

pub use search::{OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX};
pub use translate::TRANSLATE_REGEX;

use super::ScalarFunction;
use super::strings::{character, text};
use crate::error::{Code, Result, SqlError, fail};
use crate::params::ParameterType;
use crate::regex::{LobText, Regex};
use crate::value::{CharUnits, DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};
use subject::short;

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

/// [§6.33 SR8f i, SR12f i 1] The type of a string cut from, or made as
/// long as, a subject of type `subject`: CHARACTER VARYING as long as it at
/// most, or of a character large object, that type, a large object as
/// long.
fn as_long_as(name: &str, subject: &DataType) -> Result<DataType> {
    if subject.is_lob() {
        return Ok(subject.clone());
    }
    Ok(DataType::Varying { length: character(name, subject)?, units: CharUnits::Characters })
}

/// `n`, a count or position.
fn counted(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
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
        Ok(DataType::Boolean)
    }
    /// [ISO/IEC 9075-2:2023 §20.7 GR5a xxxvi] A pattern or flag CHARACTER
    /// VARYING beside the other not a `?`, the implicit flag counting; the
    /// value none, being no `<regex subject string>`.
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position, args, &[1, 2], &[])
    }
    /// \[GR1\] Unknown if any operand is null; else whether the value
    /// matches [§9.28], a character large object read a chunk at a time.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [value, pattern, flag] = args else { return fail(Code::Api, "[DESIGN] LIKE_REGEX takes a value, a pattern and a flag") };
        let (Some(pattern), Some(flag)) = (short(pattern)?, short(flag)?) else { return Ok(Value::Null) };
        let matched = match value {
            Value::Null => return Ok(Value::Null),
            Value::Lob(lob) => self.cache.compiled(&pattern, &flag)?.is_match_in(&LobText::new(lob)?)?,
            Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Row(_)
            | Value::Extension(_) => self.cache.compiled(&pattern, &flag)?.is_match(text(value)?.unwrap_or_default())?,
        };
        Ok(Value::Bool(matched))
    }
}
