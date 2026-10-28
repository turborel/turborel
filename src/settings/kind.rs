//! What values a setting takes (`Kind`), and how they are told. Split
//! from settings.rs (RULES.md R-SIZE-2).

use crate::value::Value;

/// What values a setting takes. A value is given as a `Value`: a whole
/// number as an exact number of scale 0 (or a scale whose digits are 0), a
/// number as either kind, a word or a name as a character string.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A whole number from `min` to `max`.
    Integer {
        /// The least.
        min: i128,
        /// The greatest.
        max: i128,
    },
    /// A finite number from `min` to `max`.
    Number {
        /// The least.
        min: f64,
        /// The greatest.
        max: f64,
    },
    /// One of these words, upper case, whatever case it is given in.
    Choice(&'static [&'static str]),
    /// The name of a registered storage engine.
    Engine,
    /// The name of a registered index method.
    IndexMethod,
    /// The name of a registered hash function (`Database::register_hash_function`).
    HashFunction,
    /// The names of registered encodings, by commas, in the order data
    /// passes through them; none, empty (`Database::register_encoding`).
    Encodings,
    /// An `<identifier>`'s body: at most `MAX_IDENTIFIER_LENGTH`
    /// characters, as given.
    Identifier,
    /// A time zone displacement, ±HH:MM, from -12:59 to +13:00 [ISO/IEC
    /// 9075:1992 §5.3 SR25].
    Displacement,
    /// Text of at most `max` characters, as given, its ends trimmed; it
    /// may be empty.
    Text {
        /// The most characters.
        max: usize,
    },
}

impl Kind {
    /// The values it takes, in words, as INFORMATION_SCHEMA.SETTINGS shows
    /// them and a value refused is told.
    pub fn described(&self) -> String {
        match self {
            Kind::Integer { min, max } => format!("INTEGER FROM {min} TO {max}"),
            Kind::Number { min, max } => format!("NUMBER FROM {} TO {}", Value::Approx(*min), Value::Approx(*max)),
            Kind::Choice(words) => format!("ONE OF {}", words.join(", ")),
            Kind::Engine => "A STORAGE ENGINE".to_owned(),
            Kind::IndexMethod => "AN INDEX METHOD".to_owned(),
            Kind::HashFunction => "A HASH FUNCTION".to_owned(),
            Kind::Encodings => "ENCODINGS, BY COMMAS".to_owned(),
            Kind::Identifier => "AN IDENTIFIER".to_owned(),
            Kind::Displacement => "A TIME ZONE DISPLACEMENT FROM -12:59 TO +13:00".to_owned(),
            Kind::Text { max } => format!("TEXT OF AT MOST {max} CHARACTERS"),
        }
    }
}
