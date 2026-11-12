//! [ISO/IEC 9075-2:2023 §8.7] The `<regex like predicate>`, as a function
//! of BOOLEAN that `Cond::Value` judges: whether a string matches an
//! XQuery regular expression under an option flag (`regex.rs`).

use std::sync::{Arc, Mutex};

use super::ScalarFunction;
use super::strings::{character, text};
use crate::error::{Code, Result, SqlError, fail};
use crate::params::ParameterType;
use crate::regex::Regex;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §8.7] LIKE_REGEX, no key word of SQL-92.
pub const LIKE_REGEX: &str = "LIKE_REGEX";
/// [ISO/IEC 9075-2:2023 §8.7] FLAG, no key word of SQL-92, before an
/// `<XQuery option flag>`.
pub const FLAG: &str = "FLAG";

/// A pattern and a flag, and what they compile to.
type Compiled = (String, String, Arc<Regex>);

/// CVE LIKE_REGEX PAT FLAG FL, called with CVE, PAT and FL. It keeps the
/// last pattern it compiled, so a constant one is compiled once, and one
/// from a column [Feature F847] each time it changes.
#[derive(Default)]
pub struct LikeRegex {
    last: Mutex<Option<Compiled>>,
}

impl LikeRegex {
    /// `pattern` compiled under `flag`, kept from the last call if it was
    /// the same.
    fn compiled(&self, pattern: &str, flag: &str) -> Result<Arc<Regex>> {
        let poisoned = |_| SqlError::new(Code::Api, "[DESIGN] a LIKE_REGEX's compiled pattern was left poisoned by a panic");
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
    /// [ISO/IEC 9075-2:2023 §20.7 GR5a xxxvi] A pattern or flag CHARACTER VARYING beside the
    /// other not a `?`, the implicit flag counting; the value none.
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        let other = match position {
            1 => args.get(2),
            2 => args.get(1),
            _ => None,
        };
        other.and_then(Option::as_ref).map(|_| ParameterType::Varying)
    }
    /// \[GR1\] Unknown if any operand is null; else whether the value
    /// matches [§9.28].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [value, pattern, flag] = args else { return fail(Code::Api, "[DESIGN] LIKE_REGEX takes a value, a pattern and a flag") };
        let (Some(value), Some(pattern), Some(flag)) = (text(value)?, text(pattern)?, text(flag)?) else { return Ok(Value::Null) };
        Ok(Value::Bool(self.compiled(pattern, flag)?.is_match(value)?))
    }
}
