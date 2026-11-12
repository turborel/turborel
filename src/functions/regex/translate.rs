//! [ISO/IEC 9075-2:2023 §6.33, §9.29] TRANSLATE_REGEX: a string with one
//! of an XQuery regular expression's matches, or all, replaced as
//! fn:replace replaces it.
//!
//! It is called with its units, CHARACTERS or OCTETS [Feature F846]; ALL
//! or OCCURRENCE; then the pattern, the flag, the subject, the
//! replacement, the start position and the occurrence, 1 under ALL. The
//! parser writes those not written as their implicit values [§6.33
//! SR12].

use super::{Cache, Subject, operands, parameter};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::functions::ScalarFunction;
use crate::functions::strings::{character, text, whole};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::regex::{Captures, Regex, zero_length};
use crate::value::{DataType, MAX_CHARACTER_LENGTH, Value};

/// [ISO/IEC 9075-2:2023 §6.33] TRANSLATE_REGEX, no key word of SQL-92.
pub const TRANSLATE_REGEX: &str = "TRANSLATE_REGEX";
/// Which argument of TRANSLATE_REGEX is its subject.
pub(super) const SUBJECT: usize = 4;

/// [§6.33 SR12f i 2] The maximum length of a TRANSLATE_REGEX with a
/// replacement, the longest string (IL047).
const TRANSLATED_LENGTH: u32 = MAX_CHARACTER_LENGTH;

/// TRANSLATE_REGEX ( PAT FLAG FL IN RSS WITH REP FROM SP USING CLU
/// OCCURRENCE RTO ).
#[derive(Default)]
pub(super) struct Translate {
    cache: Cache,
}

impl ScalarFunction for Translate {
    fn name(&self) -> &str {
        TRANSLATE_REGEX
    }
    /// [§6.33 SR12f] CHARACTER VARYING as long as its subject at most if
    /// the replacement is the empty string, else of the longest string.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [_, _, pattern, flag, subject, replacement, from, occurrence] = args else {
            return fail(Code::Api, "[DESIGN] TRANSLATE_REGEX takes eight operands");
        };
        let strings = [("pattern", pattern), ("flag", flag), ("subject", subject), ("replacement", replacement)];
        operands(TRANSLATE_REGEX, &strings, &[("start position", from), ("occurrence", occurrence)])?;
        let empty = character(TRANSLATE_REGEX, replacement)? == 0;
        Ok(DataType::Varying { length: if empty { character(TRANSLATE_REGEX, subject)? } else { TRANSLATED_LENGTH } })
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position, args, &[2, 3, 4, 5], &[6, 7])
    }
    /// \[GR9\] Null if an operand is, or the start position is not of the
    /// string; else the string with the match, or each match from the
    /// last, replaced.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201U of a match of no characters, 2201V of a replacement
    /// that is none [§9.29]; 22001 if the result is longer than the
    /// longest string.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [units, every, pattern, flag, subject, replacement, from, occurrence] = args else {
            return fail(Code::Api, "[DESIGN] TRANSLATE_REGEX takes eight operands");
        };
        // [GR9b i]
        let (Some(pattern), Some(flag), Some(original), Some(replacement), Some(start)) =
            (text(pattern)?, text(flag)?, text(subject)?, text(replacement)?, whole(Some(from))?)
        else {
            return Ok(Value::Null);
        };
        let counted = Subject::new(original, units)?;
        // [GR9b ii]
        let Some(at) = counted.start(start) else { return Ok(Value::Null) };
        let regex = self.cache.compiled(pattern, flag)?;
        let all = text(every)? == Some(kw::ALL);
        let chosen = if all {
            regex.list(&counted.chars, at, usize::MAX)?
        } else {
            // [GR9b iv 2 A]
            let Some(occurrence) = whole(Some(occurrence))? else { return Ok(Value::Null) };
            let Some(occurrence) = usize::try_from(occurrence).ok().filter(|o| *o >= 1) else {
                return Ok(Value::Char(original.to_owned()));
            };
            regex.list(&counted.chars, at, occurrence)?.into_iter().nth(occurrence.saturating_sub(1)).into_iter().collect()
        };
        let mut chars = counted.chars;
        // [GR9b iv 2 B] from the last match to the first.
        for mv in chosen.iter().rev() {
            chars = replaced(&regex, &chars, mv, replacement)?;
        }
        within_most(chars)
    }
}

/// [§9.29] `chars` with the match `mv` replaced by `replacement`, as
/// fn:replace replaces that match alone.
///
/// # Errors
///
/// SQLSTATE 2201U if it matched no characters \[GR3\].
fn replaced(regex: &Regex, chars: &[char], mv: &Captures, replacement: &str) -> Result<Vec<char>> {
    let Some((start, end)) = mv.get(0).filter(|(start, end)| end > start) else {
        return Err(zero_length("a match of the pattern is of no characters"));
    };
    let t = regex.replace(chars.get(start..end).unwrap_or_default(), replacement)?;
    let mut out: Vec<char> = chars.get(..start).unwrap_or_default().to_vec();
    out.extend(t.chars());
    out.extend(chars.get(end..).unwrap_or_default());
    Ok(out)
}

/// `chars`, if no longer than the longest string.
///
/// # Errors
///
/// SQLSTATE 22001 if they are longer (IL047).
fn within_most(chars: Vec<char>) -> Result<Value> {
    if u32::try_from(chars.len()).is_ok_and(|n| n <= TRANSLATED_LENGTH) {
        return Ok(Value::Char(chars.into_iter().collect()));
    }
    fail_with(
        Code::Data,
        SqlState::STRING_DATA_RIGHT_TRUNCATION,
        format!("[ISO/IEC 9075-2:2023 §6.33 SR12f i 2] TRANSLATE_REGEX of {} characters is longer than {TRANSLATED_LENGTH}", chars.len()),
    )
}
