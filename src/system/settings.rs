//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.SETTINGS:
//! the settings (RULES.md D36), an object "associated with
//! INFORMATION_SCHEMA" this implementation adds. Split from system.rs
//! (RULES.md R-SIZE-2).

use super::{Seen, SystemTable, identifier, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::settings::{Effect, Kind, Source};
use crate::storage::Row;
use crate::value::Value;

/// How long a value or default of the SETTINGS table may be; one longer is
/// cut to it.
const VALUE_LENGTH: u32 = 256;
/// How long a setting's description may be (`Settings::declare` refuses one longer).
pub const DESCRIPTION_LENGTH: usize = 512;

/// \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.SETTINGS:
/// every setting (RULES.md D36), with the value the session sees, where it
/// comes from, its default, the values it takes, whether a session may set
/// it, when a change takes effect, and what it does.
pub(super) struct Settings;

impl SystemTable for Settings {
    fn name(&self) -> &'static str {
        "SETTINGS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let description = u32::try_from(DESCRIPTION_LENGTH).unwrap_or(u32::MAX);
        vec![
            identifier("SETTING_NAME", true),
            text_column(("SETTING_VALUE", VALUE_LENGTH), true),
            text_column(("VALUE_SOURCE", 8), true),
            text_column(("DEFAULT_VALUE", VALUE_LENGTH), true),
            text_column(("VALUE_KIND", VALUE_LENGTH), true),
            text_column(("IS_SESSION_SETTABLE", 3), true),
            text_column(("TAKES_EFFECT", 11), true),
            text_column(("DESCRIPTION", description), true),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let shown = |v: &Value| Value::Char(v.to_string().chars().take(usize::try_from(VALUE_LENGTH).unwrap_or(usize::MAX)).collect());
        Ok(seen
            .state
            .settings
            .list(seen.own)
            .into_iter()
            .map(|l| {
                let d = &l.definition;
                let source = match l.source {
                    Source::Default => "DEFAULT",
                    Source::Database => "DATABASE",
                    Source::Session => "SESSION",
                };
                let effect = match d.effect {
                    Effect::Statement => "STATEMENT",
                    Effect::Transaction => "TRANSACTION",
                    Effect::Start => "START",
                    Effect::Creation => "CREATION",
                };
                vec![
                    text(&d.name),
                    shown(&l.value),
                    text(source),
                    shown(&d.default),
                    shown(&Value::Char(kind_of(&d.kind))),
                    yes(d.per_session),
                    text(effect),
                    text(&d.doc),
                ]
            })
            .collect())
    }
}

/// The values a setting of `kind` takes, as the SETTINGS table shows them.
pub(super) fn kind_of(kind: &Kind) -> String {
    match kind {
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
