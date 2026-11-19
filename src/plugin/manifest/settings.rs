//! \[EXTENSION\] The settings a plug-in's manifest declares and reads
//! (RULES.md D280, D281): each `SETTING` a setting of the database's,
//! named under its extension's name, of a kind the settings system has
//! (`settings::Kind`), and `READS SETTINGS` the built-in settings its
//! program is told besides its own, read-only.
//!
//! ```text
//! <setting declaration> ::= SETTING <identifier> TYPE <setting type>
//!     [ MIN <setting literal> ] [ MAX <setting literal> ]
//!     [ CHOICES ( <identifier> [ , ... ] ) ] [ DEFAULT <setting literal> ]
//!     [ SCOPE { SESSION | DATABASE | NODE } ] [ DESCRIPTION <character string literal> ]
//! <setting type>    ::= INTEGER | NUMBER | CHOICE | SWITCH | TEXT | IDENTIFIER
//! <setting literal> ::= <signed numeric literal> | <character string literal>
//! <reads settings>  ::= READS SETTINGS ( <character string literal> [ , ... ] )
//! ```

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::error::Result;
use crate::settings::{Definition, Effect, Kind, OFF, SWITCHES, accepted};
use crate::value::{Decimal, Value};

use super::super::errors::invalid_manifest;

/// The most characters of a TEXT setting with no MAX: as many as a
/// setting's description holds.
const TEXT_MOST: usize = 4000;
/// The least and greatest whole numbers of an INTEGER with no MIN or MAX:
/// those of a BIGINT, as a program reads them.
const INTEGER_RANGE: (i128, i128) = (-9_223_372_036_854_775_808, 9_223_372_036_854_775_807);

/// The values a declared setting takes, as its TYPE names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingType {
    /// A whole number, from MIN to MAX.
    Integer,
    /// A finite number, from MIN to MAX.
    Number,
    /// One of its CHOICES.
    Choice,
    /// ON or OFF.
    Switch,
    /// Text of at most MAX characters.
    Text,
    /// An `<identifier>`'s body.
    Identifier,
}

impl SettingType {
    /// Every type, by the word that names it.
    pub const ALL: [(&'static str, SettingType); 6] = [
        ("INTEGER", SettingType::Integer),
        ("NUMBER", SettingType::Number),
        ("CHOICE", SettingType::Choice),
        ("SWITCH", SettingType::Switch),
        ("TEXT", SettingType::Text),
        ("IDENTIFIER", SettingType::Identifier),
    ];
}

/// Who sets a declared setting, as its SCOPE names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// A session for itself, over the database's value.
    Session,
    /// The database, its value logged with the definitions.
    Database,
    /// The node, given by the host, never logged.
    Node,
}

impl Scope {
    /// Every scope, by the word that names it.
    pub const ALL: [(&'static str, Scope); 3] = [("SESSION", Scope::Session), ("DATABASE", Scope::Database), ("NODE", Scope::Node)];
}

/// A setting a manifest declares, as written.
#[derive(Debug, Clone)]
pub struct SettingDeclared {
    /// Its name, without its extension's.
    pub name: String,
    /// Its TYPE.
    pub kind: SettingType,
    /// Its MIN, if written.
    pub min: Option<Value>,
    /// Its MAX, if written.
    pub max: Option<Value>,
    /// Its CHOICES, upper case.
    pub choices: Vec<String>,
    /// Its DEFAULT, if written.
    pub default: Option<Value>,
    /// Its SCOPE, if written.
    pub scope: Option<Scope>,
    /// Its DESCRIPTION, if written.
    pub description: Option<String>,
    /// The declaration as written.
    pub text: String,
}

impl SettingDeclared {
    /// A declaration of `name` and `kind`, of nothing more yet.
    pub fn new(name: String, kind: SettingType) -> SettingDeclared {
        SettingDeclared { name, kind, min: None, max: None, choices: Vec::new(), default: None, scope: None, description: None, text: String::new() }
    }

    /// Its name under its extension `extension`'s: `EXTENSION.NAME`.
    pub fn qualified(&self, extension: &str) -> String {
        format!("{}.{}", extension.to_ascii_uppercase(), self.name.to_ascii_uppercase())
    }

    /// Its scope: as written, else NODE of a host plug-in's and DATABASE
    /// of an extension's.
    pub fn scope_of(&self, host: bool) -> Scope {
        self.scope.unwrap_or(if host { Scope::Node } else { Scope::Database })
    }

    /// The setting it declares for the extension `extension`, a host
    /// plug-in if `host`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if its clauses do not fit its type, a host plug-in's
    /// is not the node's, or its default is not one of its values.
    pub fn definition(&self, extension: &str, host: bool) -> Result<Definition> {
        let name = self.qualified(extension);
        let wrong = |why: &str| invalid_manifest(&format!("SETTING {}: {why}", self.name));
        let scope = self.scope_of(host);
        if host && scope != Scope::Node {
            return Err(wrong("a host plug-in's settings are the node's, SCOPE NODE (RULES.md D281)"));
        }
        let kind = self.kind_of().map_err(|why| wrong(&why))?;
        let default = self.default.clone().map_or_else(|| first_value(&kind), Ok).map_err(|why| wrong(&why))?;
        let default = accepted(&name, &kind, &default).map_err(|e| wrong(&e.msg))?;
        let doc = self.description.clone().unwrap_or_else(|| format!("A setting of the plug-in {}.", extension.to_ascii_uppercase()));
        Ok(Definition {
            name,
            kind,
            default,
            per_session: scope == Scope::Session,
            node: scope == Scope::Node,
            effect: Effect::Statement,
            doc,
        })
    }

    /// The settings system's kind of it, or why its clauses do not fit.
    fn kind_of(&self) -> std::result::Result<Kind, String> {
        let ranged = matches!(self.kind, SettingType::Integer | SettingType::Number | SettingType::Text);
        if !ranged && (self.min.is_some() || self.max.is_some()) {
            return Err("only an INTEGER, a NUMBER or a TEXT takes MIN or MAX".to_owned());
        }
        if (self.kind == SettingType::Choice) == self.choices.is_empty() {
            return Err("a CHOICE takes CHOICES, and only a CHOICE does".to_owned());
        }
        Ok(match self.kind {
            SettingType::Integer => {
                let min = self.min.as_ref().map_or(Some(INTEGER_RANGE.0), whole).ok_or("MIN is no whole number")?;
                let max = self.max.as_ref().map_or(Some(INTEGER_RANGE.1), whole).ok_or("MAX is no whole number")?;
                ordered(min, max, Kind::Integer { min, max })?
            }
            SettingType::Number => {
                let min = self.min.as_ref().map_or(Some(f64::MIN), Value::as_f64).ok_or("MIN is no number")?;
                let max = self.max.as_ref().map_or(Some(f64::MAX), Value::as_f64).ok_or("MAX is no number")?;
                if min > max { return Err("MIN is greater than MAX".to_owned()) } else { Kind::Number { min, max } }
            }
            SettingType::Text => {
                if self.min.is_some() {
                    return Err("a TEXT takes MAX alone".to_owned());
                }
                let max = self.max.as_ref().map_or(Some(TEXT_MOST), |m| whole(m).and_then(|n| usize::try_from(n).ok()));
                Kind::Text { max: max.ok_or("MAX is no count of characters")? }
            }
            SettingType::Choice => Kind::Choice(kept_words(&self.choices)),
            SettingType::Switch => Kind::Choice(SWITCHES),
            SettingType::Identifier => Kind::Identifier,
        })
    }
}

/// `kind`, if `min` is not past `max`.
fn ordered(min: i128, max: i128, kind: Kind) -> std::result::Result<Kind, String> {
    if min > max { Err("MIN is greater than MAX".to_owned()) } else { Ok(kind) }
}

/// The whole number `v` is, if it is one.
fn whole(v: &Value) -> Option<i128> {
    let Value::Exact(d) = v else { return None };
    let unit = crate::value::pow10(d.scale)?;
    d.units.checked_div(unit).filter(|n| n.checked_mul(unit) == Some(d.units))
}

/// The value a setting of `kind` has when its manifest writes no DEFAULT:
/// 0 or the end of its range nearest it, the first choice, OFF, or empty
/// text. An identifier has none, as it is never empty.
fn first_value(kind: &Kind) -> std::result::Result<Value, String> {
    match kind {
        Kind::Integer { min, max } => Ok(Value::Exact(Decimal::new(0_i128.clamp(*min, *max), 0))),
        Kind::Number { min, max } => Ok(Value::Approx(0.0_f64.clamp(*min, *max))),
        Kind::Choice(words) => {
            let word = if *words == SWITCHES { OFF } else { words.first().copied().unwrap_or(OFF) };
            Ok(Value::Char(word.to_owned()))
        }
        Kind::Text { .. } => Ok(Value::Char(String::new())),
        Kind::Identifier
        | Kind::Engine
        | Kind::IndexMethod
        | Kind::HashFunction
        | Kind::Encodings
        | Kind::Displacement
        | Kind::Columns { .. } => Err("it must write a DEFAULT".to_owned()),
    }
}

/// `words`, kept as long as the program runs, as a choice's words are
/// `&'static`; each list kept once, however often an extension is made
/// again.
fn kept_words(words: &[String]) -> &'static [&'static str] {
    static KEPT: OnceLock<Mutex<BTreeMap<Vec<String>, &'static [&'static str]>>> = OnceLock::new();
    let mut kept = crate::sync::lock_builtin(KEPT.get_or_init(Mutex::default));
    if let Some(found) = kept.get(words) {
        return found;
    }
    let made: Vec<&'static str> = words.iter().map(|w| crate::plugin::bridge::leaked(w)).collect();
    let made: &'static [&'static str] = Box::leak(made.into_boxed_slice());
    kept.insert(words.to_vec(), made);
    made
}

/// The rules of a manifest's settings and the built-in settings it
/// reads: no setting declared twice, nor read twice, and each read one
/// built in.
///
/// # Errors
///
/// SQLSTATE 38T06 of the first broken.
pub fn check(name: &str, declared: &[SettingDeclared], reads: &[String], host: bool) -> Result<()> {
    let mut seen = HashSet::new();
    for d in declared {
        if !seen.insert(d.name.to_ascii_uppercase()) {
            return Err(invalid_manifest(&format!("SETTING {} is declared twice", d.name)));
        }
        d.definition(name, host)?;
    }
    let builtin = crate::settings::Settings::default();
    let mut read = HashSet::new();
    for r in reads {
        if !read.insert(r.to_ascii_uppercase()) {
            return Err(invalid_manifest(&format!("READS SETTINGS names {r} twice")));
        }
        if builtin.key(r).is_none() {
            return Err(invalid_manifest(&format!("READS SETTINGS names {r}, which is no built-in setting")));
        }
    }
    Ok(())
}
