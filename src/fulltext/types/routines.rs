//! The FullText methods [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.9],
//! the Language attribute's observer [§5.1.1 Description 1a] and
//! FullText_to_Character [§5.1.10], as scalar functions called with the
//! function-call syntax: FULLTEXT(string [, language]), LANGUAGE(SELF) and
//! FULLTEXT_TO_CHARACTER(text).
//!
//! The FullText value of a null string is, by the draft, one of a null
//! Contents [§5.1.9 Definition]. \[DESIGN\] It is the null value here: every
//! public routine of a value of a null Contents gives what it gives of a
//! null value [§5.1.2 Description 3c i, §5.1.3 Description 3b i, §5.1.10].

use super::fulltext::parts;
use super::{FT_DEFAULT_LANGUAGE, FT_MAX_LANGUAGE_LENGTH, FT_MAX_TEXT_LENGTH, FullTextType, full_text, is_type};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::fulltext::Language;
use crate::fulltext::value::FULLTEXT;
use crate::functions::ScalarFunction;
use crate::value::{DataType, MAX_CHARACTER_LENGTH, Value};

/// The name of `FullTextRoutine::ToCharacter`.
pub const FULLTEXT_TO_CHARACTER: &str = "FULLTEXT_TO_CHARACTER";

/// The name of `FullTextRoutine::Language`.
pub const LANGUAGE: &str = "LANGUAGE";

/// The arguments of the FullText method of a language.
const WITH_LANGUAGE: usize = 2;

/// The routines of this file, each a scalar function.
pub const ROUTINES: [FullTextRoutine; 3] = [FullTextRoutine::Construct, FullTextRoutine::ToCharacter, FullTextRoutine::Language];

/// A FullText routine.
#[derive(Debug, Clone, Copy)]
pub enum FullTextRoutine {
    /// FULLTEXT(string [, language]), the FullText methods [§5.1.9]: the
    /// value of `string`, in `language`, else in FT_DefaultLanguage.
    Construct,
    /// FULLTEXT_TO_CHARACTER(text), its text [§5.1.10].
    ToCharacter,
    /// LANGUAGE(SELF), its language's name [§5.1.1 Description 1a].
    Language,
}

/// The error of a call of `name` not of the arguments it takes.
fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-2:2002 §5.1.1] invalid arguments to {name}"))
}

/// The error of a language that is null or empty [§5.1.9 Description 3].
fn no_language() -> SqlError {
    SqlError::new(Code::Data, "[ISO/IEC 13249-2:2002 §5.1.9] a language specification is null or empty")
        .with_state(SqlState::INVALID_LANGUAGE_SPECIFICATION)
}

/// The language `v` names, its trailing spaces not counting.
///
/// # Errors
///
/// SQLSTATE 2FF02 if it is null, empty or names no language the tokenizer
/// knows [§5.1.9 Description 3].
fn language_named(v: &Value) -> Result<Language> {
    let Value::Char(name) = v else { return Err(no_language()) };
    let name = name.trim_end_matches(' ');
    if name.is_empty() {
        return Err(no_language());
    }
    Language::named(name)
}

impl FullTextRoutine {
    /// Whether it takes `count` arguments.
    fn arity(self, count: usize) -> bool {
        match self {
            Self::Construct => (1..=WITH_LANGUAGE).contains(&count),
            Self::ToCharacter | Self::Language => count == 1,
        }
    }

    /// The result's type.
    fn output(self) -> DataType {
        match self {
            Self::Construct => FullTextType::data_type(),
            Self::ToCharacter => DataType::varying(FT_MAX_TEXT_LENGTH),
            Self::Language => DataType::varying(FT_MAX_LANGUAGE_LENGTH),
        }
    }

    /// Whether `t` is the type of its argument: a character string of the
    /// FullText method, a FullText value of the others.
    fn takes(self, t: &DataType) -> bool {
        match self {
            Self::Construct => t.is_character(),
            Self::ToCharacter | Self::Language => is_type(t, FULLTEXT),
        }
    }

    /// The FullText method [§5.1.9]: of a language, checked first, as the
    /// method signals before it makes a value; of a null string, null.
    fn construct(args: &[Value]) -> Result<Value> {
        let (string, language) = match args {
            [string] => (string, FT_DEFAULT_LANGUAGE),
            [string, language] => (string, language_named(language)?),
            _ => return Err(signature(FULLTEXT)),
        };
        let Value::Char(text) = string else {
            return if string.is_null() { Ok(Value::Null) } else { Err(signature(FULLTEXT)) };
        };
        full_text(language, text)
    }
}

impl ScalarFunction for FullTextRoutine {
    fn name(&self) -> &'static str {
        match self {
            Self::Construct => FULLTEXT,
            Self::ToCharacter => FULLTEXT_TO_CHARACTER,
            Self::Language => LANGUAGE,
        }
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if self.arity(args.len()) && args.iter().all(|t| self.takes(t)) { Ok(self.output()) } else { Err(signature(self.name())) }
    }

    /// The FullText method's string is a character string of
    /// FT_MaxTextLength, its language one of the longest, so a name too
    /// long is no language (2FF02), not one cut short; the others' is a
    /// FullText value.
    fn argument_type(&self, position: usize, _: &[Option<DataType>]) -> Option<DataType> {
        match (self, position) {
            (Self::Construct, 0) => Some(DataType::varying(FT_MAX_TEXT_LENGTH)),
            (Self::Construct, _) => Some(DataType::varying(MAX_CHARACTER_LENGTH)),
            (Self::ToCharacter | Self::Language, _) => Some(FullTextType::data_type()),
        }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        if !self.arity(args.len()) {
            return Err(signature(self.name()));
        }
        if matches!(self, Self::Construct) {
            return Self::construct(args);
        }
        let [text] = args else { return Err(signature(self.name())) };
        if text.is_null() {
            return Ok(Value::Null);
        }
        let (language, contents) = parts(text)?;
        Ok(Value::Char(if matches!(self, Self::Language) { language.name().into() } else { contents.into() }))
    }

    fn deterministic(&self) -> bool {
        true
    }
}
