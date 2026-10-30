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
    /// characters, not empty, as given, its ends trimmed.
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
    /// \[EXTENSION\] Of a table option only: names of the table's columns,
    /// by commas, at most `most` of them, none twice; none, empty. A column
    /// named is not dropped (`db/alter/columns.rs`).
    Columns {
        /// The most columns.
        most: usize,
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
            Kind::Columns { most } => format!("AT MOST {most} OF THE TABLE'S COLUMNS, BY COMMAS"),
        }
    }
}

/// \[EXTENSION\] The column names of `text`, a value of `Kind::Columns`,
/// by commas: each an `<identifier>` as SQL reads one, a regular one upper
/// case, a delimited one, in double quotes, as written, a quote in it
/// doubled [ISO/IEC 9075:1992 §5.2 SR10, SR13, SR14]; none if it is malformed.
pub fn column_names(text: &str) -> Option<Vec<String>> {
    let mut names = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        // At the end, only if no comma came before.
        if chars.peek().is_none() {
            return names.is_empty().then_some(names);
        }
        let mut name = String::new();
        if chars.next_if_eq(&'"').is_some() {
            loop {
                let c = chars.next()?;
                if c != '"' {
                    name.push(c);
                } else if chars.next_if_eq(&'"').is_some() {
                    name.push('"');
                } else {
                    break;
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| *c != ',') {
                name.push(c);
            }
            name = name.trim().to_uppercase();
        }
        if name.is_empty() {
            return None;
        }
        names.push(name);
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.next() {
            None => return Some(names),
            Some(',') => {}
            Some(_) => return None,
        }
    }
}

/// \[EXTENSION\] `names` as a value of `Kind::Columns`: by commas, each
/// regular one as it is, each other delimited.
pub fn column_list(names: &[String]) -> String {
    let written: Vec<String> = names
        .iter()
        .map(|n| {
            let regular = n.chars().next().is_some_and(char::is_alphabetic)
                && n.chars().all(|c| c.is_alphanumeric() || c == '_')
                && *n == n.to_uppercase();
            if regular { n.clone() } else { format!("\"{}\"", n.replace('"', "\"\"")) }
        })
        .collect();
    written.join(", ")
}
