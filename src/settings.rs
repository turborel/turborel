//! \[DESIGN\] Settings (RULES.md D36): the registry of every setting the
//! database has, each declared once, with its kind, its default, whether a
//! session may set it for itself, and when a change takes effect; and the
//! values set, the database's and a session's own.
//!
//! The built-in settings are declared here; a plug-in declares its own
//! (`Database::declare_setting`). A database setting is a definition,
//! logged and replayed with the others (RULES.md D25), so it survives the
//! database being closed; one replayed before the plug-in that declares it
//! is kept until it is declared. A session's settings are its own, and end
//! with it. A name is an SQL `<identifier>`, upper case, its parts joined by
//! periods, a plug-in's under its own name ("ROWSTORE.X").

mod builtin;

use std::sync::Arc;

use crate::error::{Code, Result, SqlError};
use crate::txn::{LockWait, Policy, SessionEnd};
use crate::value::{Value, pow10};

pub use builtin::{
    CATALOG_COMPACT_AFTER, CATALOG_NAME, CHECKPOINT_AFTER_BYTES, CHECKPOINT_AFTER_CHANGES, CHECKSUM, DECORRELATE_AFTER, DEFAULT_ENGINE,
    ENCODINGS, EQUAL_SELECTIVITY, INDEX_DESCENT_COST, LOCK_TIMEOUT, LOCK_WAIT, LOG_SEGMENT_SIZE, MEMORY_BUDGET, NULL_FRACTION,
    OPTIMIZER_MAX_PASSES, PAGE_SIZE, POOL_BYTES, RANGE_SELECTIVITY, REORDER_GAIN, SESSION_END, SLOW_STATEMENT, SORT_COMPARE_COST,
    SPILL_BATCH, SPILL_PARTITION_DEPTH, SPILL_PARTITIONS, STATEMENT_TIMEOUT, TRANSACTION_POLICY, UNIQUE_INDEX_METHOD, WORK_MEMORY,
};
use builtin::{builtin, whole};

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
    /// Text of at most `max` characters, as given, its ends trimmed; it
    /// may be empty.
    Text {
        /// The most characters.
        max: usize,
    },
}

/// When a change of a setting takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// For every statement that begins after it.
    Statement,
    /// For every transaction that begins after it.
    Transaction,
    /// When what reads it starts next: a server, as it starts listening; a
    /// database kept in a directory, as it is opened.
    Start,
    /// When the database is created in its directory: given before, and
    /// fixed after, kept in its files' formats.
    Creation,
}

/// A setting, as declared.
#[derive(Debug, Clone)]
pub struct Definition {
    /// Its name.
    pub name: String,
    /// The values it takes.
    pub kind: Kind,
    /// Its value until it is set, of its kind.
    pub default: Value,
    /// Whether a session may set it for itself, over the database's value.
    pub per_session: bool,
    /// When a change takes effect.
    pub effect: Effect,
    /// What it does, one sentence.
    pub doc: String,
}

/// \[DESIGN\] A setting of the registry: its place, found from its name once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key(usize);

/// Where a setting's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Its default: it is not set.
    Default,
    /// The database's.
    Database,
    /// The session's own.
    Session,
}

/// A setting as a listing shows it: its definition, its value, and where
/// the value comes from.
#[derive(Debug, Clone)]
pub struct Listed {
    /// The setting.
    pub definition: Definition,
    /// Its value.
    pub value: Value,
    /// Where the value comes from.
    pub source: Source,
}

/// The bytes a value of MEMORY_BUDGET says.
pub fn budget_bytes(value: &Value) -> usize {
    bytes(value, crate::memory::DEFAULT_BUDGET)
}

/// The bytes a value of WORK_MEMORY says.
pub fn work_bytes(value: &Value) -> usize {
    bytes(value, crate::spill::DEFAULT_WORK_MEMORY)
}

/// The bytes a value of SPILL_BATCH says.
pub fn batch_bytes(value: &Value) -> usize {
    bytes(value, crate::spill::DEFAULT_SPILL_BATCH)
}

/// The bytes a whole number `value` says, or `default` if it is not one.
fn bytes(value: &Value, default: usize) -> usize {
    match value {
        Value::Exact(d) => usize::try_from(d.units).unwrap_or(usize::MAX),
        Value::Approx(_) | Value::Char(_) | Value::Null | Value::Extension(_) => default,
    }
}

/// An error of a setting: an API misuse (SQLCODE -910).
fn misuse(msg: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {msg}"))
}

/// `name` as a setting's name: upper case, without surrounding spaces.
fn canonical(name: &str) -> String {
    name.trim().to_ascii_uppercase()
}

/// `value` as a value of `kind`, for the setting `name`: an exact number of
/// scale 0, an approximate number, an upper-case word, or a name.
///
/// # Errors
///
/// SQLCODE -910 if it is not of the kind, or out of its range.
pub(crate) fn accepted(name: &str, kind: &Kind, value: &Value) -> Result<Value> {
    let refused = || misuse(&format!("{name} takes {kind:?}, not {value}"));
    let given = if let Value::Char(s) = value { Some(s.as_str()) } else { None };
    match kind {
        Kind::Integer { min, max } => {
            let Value::Exact(d) = value else { return Err(refused()) };
            let unit = pow10(d.scale).ok_or_else(refused)?;
            let n = d.units.checked_div(unit).filter(|n| n.checked_mul(unit) == Some(d.units)).ok_or_else(refused)?;
            if n < *min || n > *max {
                return Err(refused());
            }
            Ok(whole(n))
        }
        // A character string is no number: `as_f64` gives it none.
        Kind::Number { min, max } => {
            let x = value.as_f64().filter(|x| x.is_finite() && *x >= *min && *x <= *max).ok_or_else(refused)?;
            Ok(Value::Approx(x))
        }
        Kind::Choice(words) => {
            let word = given.map(canonical).filter(|w| words.contains(&w.as_str())).ok_or_else(refused)?;
            Ok(Value::Char(word))
        }
        Kind::Engine | Kind::IndexMethod | Kind::HashFunction => given.map(|s| Value::Char(s.trim().to_owned())).ok_or_else(refused),
        Kind::Encodings => {
            let names: Vec<&str> = given.ok_or_else(refused)?.split(',').map(str::trim).filter(|n| !n.is_empty()).collect();
            Ok(Value::Char(names.join(",")))
        }
        Kind::Identifier => {
            let body = given.map(str::trim).filter(|s| !s.is_empty() && s.chars().count() <= crate::lexer::MAX_IDENTIFIER_LENGTH);
            body.map(|s| Value::Char(s.to_owned())).ok_or_else(refused)
        }
        Kind::Text { max } => {
            given.map(str::trim).filter(|s| s.chars().count() <= *max).map(|s| Value::Char(s.to_owned())).ok_or_else(refused)
        }
    }
}

/// \[DESIGN\] The registry and the database's values: shared by the
/// snapshots of the definitions (`db/state.rs`), changed by copying.
#[derive(Debug, Clone)]
pub struct Settings {
    definitions: Arc<Vec<Definition>>,
    /// The database's value of each setting, at its key's place; none for
    /// its default.
    values: Vec<Option<Value>>,
    /// Values set before their settings were declared: replayed from the
    /// log before the plug-in declaring them was registered.
    pending: Vec<(String, Value)>,
}

impl Default for Settings {
    fn default() -> Self {
        let definitions = builtin();
        let values = vec![None; definitions.len()];
        Settings { definitions: Arc::new(definitions), values, pending: Vec::new() }
    }
}

impl Settings {
    /// The key of the setting named `name`, in any case.
    pub fn key(&self, name: &str) -> Option<Key> {
        let name = canonical(name);
        self.definitions.iter().position(|d| d.name == name).map(Key)
    }

    /// The key of `name`, which must be declared.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is not.
    pub fn known(&self, name: &str) -> Result<Key> {
        self.key(name).ok_or_else(|| misuse(&format!("there is no setting {}", canonical(name))))
    }

    /// The definition of `key`.
    pub fn definition(&self, key: Key) -> Option<&Definition> {
        self.definitions.get(key.0)
    }

    /// Declares the setting `definition`, taking a value set for it before.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its name is declared, or its default, or the
    /// value set before, is not of its kind.
    pub fn declare(&mut self, mut definition: Definition) -> Result<Key> {
        definition.name = canonical(&definition.name);
        let (named, described) = (definition.name.chars().count(), definition.doc.chars().count());
        if named > crate::lexer::MAX_IDENTIFIER_LENGTH || described > crate::system::DESCRIPTION_LENGTH {
            return Err(misuse(&format!(
                "a setting's name is at most {} characters, and its description at most {}",
                crate::lexer::MAX_IDENTIFIER_LENGTH,
                crate::system::DESCRIPTION_LENGTH
            )));
        }
        if self.key(&definition.name).is_some() {
            return Err(misuse(&format!("{} is declared twice", definition.name)));
        }
        definition.default = accepted(&definition.name, &definition.kind, &definition.default)?;
        let earlier = self.pending.iter().position(|(n, _)| *n == definition.name).map(|at| self.pending.remove(at).1);
        let value = earlier.map(|v| accepted(&definition.name, &definition.kind, &v)).transpose()?;
        Arc::make_mut(&mut self.definitions).push(definition);
        self.values.push(value);
        Ok(Key(self.values.len().saturating_sub(1)))
    }

    /// The database's value of `key`, or its default.
    pub fn get(&self, key: Key) -> &Value {
        match self.values.get(key.0) {
            Some(Some(v)) => v,
            Some(None) | None => self.definition(key).map_or(&Value::Null, |d| &d.default),
        }
    }

    /// Sets the database's value of the setting `name`, or, for none, its
    /// default again. One not declared keeps the value until it is.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the value is not of the setting's kind.
    pub fn set(&mut self, name: &str, value: Option<&Value>) -> Result<()> {
        let Some(key) = self.key(name) else {
            let name = canonical(name);
            self.pending.retain(|(n, _)| *n != name);
            if let Some(v) = value {
                self.pending.push((name, v.clone()));
            }
            return Ok(());
        };
        let accepted = match (self.definition(key), value) {
            (Some(d), Some(v)) => Some(accepted(&d.name, &d.kind, v)?),
            (Some(_), None) => None,
            (None, v) => v.cloned(),
        };
        if let Some(slot) = self.values.get_mut(key.0) {
            *slot = accepted;
        }
        Ok(())
    }

    /// The database's values of the settings read as the database opens
    /// (`Effect::Start`), each with its setting's name.
    pub(crate) fn started(&self) -> Vec<(String, Value)> {
        let set = self.definitions.iter().zip(&self.values).filter(|(d, _)| d.effect == Effect::Start);
        set.filter_map(|(d, v)| Some((d.name.clone(), v.clone()?))).collect()
    }

    /// The database's values, each with its setting's name: those of the
    /// settings declared, then those set before theirs were.
    pub(crate) fn database_values(&self) -> Vec<(String, Value)> {
        let set = self.definitions.iter().zip(&self.values).filter_map(|(d, v)| Some((d.name.clone(), v.clone()?)));
        set.chain(self.pending.iter().cloned()).collect()
    }

    /// The value of `key` with a session's `own` values over the
    /// database's, and where it comes from.
    pub fn effective<'a>(&'a self, key: Key, own: &'a Own) -> (&'a Value, Source) {
        if let Some(v) = own.get(key) {
            return (v, Source::Session);
        }
        let source = if self.values.get(key.0).is_some_and(Option::is_some) { Source::Database } else { Source::Default };
        (self.get(key), source)
    }

    /// Every setting, with its value as a session with `own` values sees it.
    pub fn list(&self, own: &Own) -> Vec<Listed> {
        (0..self.definitions.len())
            .filter_map(|k| {
                let key = Key(k);
                let (value, source) = self.effective(key, own);
                self.definition(key).map(|d| Listed { definition: d.clone(), value: value.clone(), source })
            })
            .collect()
    }

    /// The whole number of `key`, 0 if it is none.
    pub fn integer(&self, key: Key) -> i128 {
        if let Value::Exact(d) = self.get(key) { d.units } else { 0 }
    }

    /// The same registry and values, each of `own`'s values over the
    /// database's: what a session's statement reads (RULES.md D36).
    #[must_use]
    pub fn overlaid(&self, own: &Own) -> Settings {
        let mut out = self.clone();
        for (key, value) in &own.0 {
            if let Some(slot) = out.values.get_mut(key.0) {
                *slot = Some(value.clone());
            }
        }
        out
    }

    /// The whole number of `key` as a count: 0 if it is none or below 0,
    /// `usize::MAX` if it is more.
    pub fn count(&self, key: Key) -> usize {
        usize::try_from(self.integer(key).max(0)).unwrap_or(usize::MAX)
    }

    /// The number of `key`, 0 if it is none.
    pub fn number(&self, key: Key) -> f64 {
        self.get(key).as_f64().unwrap_or_default()
    }

    /// The word or name of `key`, empty if it is none.
    pub fn word(&self, key: Key) -> &str {
        word_of(self.get(key))
    }

    /// [X3.135-1986 §4.16] The transaction policy.
    pub fn policy(&self) -> Policy {
        if self.word(TRANSACTION_POLICY) == "PERMISSIVE" { Policy::Permissive } else { Policy::Strict }
    }
}

/// The word or name a value is, empty if it is none.
fn word_of(v: &Value) -> &str {
    if let Value::Char(s) = v { s } else { "" }
}

/// [X3.135-1986 §7.1 GR2] The session end a value says.
pub fn session_end(v: &Value) -> SessionEnd {
    if word_of(v) == "ROLLBACK" { SessionEnd::Rollback } else { SessionEnd::Commit }
}

/// The lock wait a value says.
pub fn lock_wait(v: &Value) -> LockWait {
    if word_of(v) == "WAIT" { LockWait::Wait } else { LockWait::NoWait }
}

/// The word of `LOCK_WAIT` for `wait`.
pub fn lock_wait_word(wait: LockWait) -> &'static str {
    match wait {
        LockWait::NoWait => "NO_WAIT",
        LockWait::Wait => "WAIT",
    }
}

/// The word of `SESSION_END` for `end`.
pub fn session_end_word(end: SessionEnd) -> &'static str {
    match end {
        SessionEnd::Commit => "COMMIT",
        SessionEnd::Rollback => "ROLLBACK",
    }
}

/// The word of `TRANSACTION_POLICY` for `policy`.
pub fn policy_word(policy: Policy) -> &'static str {
    match policy {
        Policy::Strict => "STRICT",
        Policy::Permissive => "PERMISSIVE",
    }
}

/// A session's own values of the settings it may set for itself.
#[derive(Debug, Clone, Default)]
pub struct Own(Vec<(Key, Value)>);

/// No values of one's own: a statement of no session's.
pub static NONE: Own = Own(Vec::new());

impl Own {
    /// Whether it set no value.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Its value of `key`, if it set one.
    pub fn get(&self, key: Key) -> Option<&Value> {
        self.0.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// Sets its value of the setting `name` of `settings`, or, for none,
    /// takes the database's again.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such setting, a session may not set it,
    /// or the value is not of its kind.
    pub fn set(&mut self, settings: &Settings, name: &str, value: Option<&Value>) -> Result<()> {
        let key = settings.known(name)?;
        let definition = settings.definition(key).ok_or_else(|| misuse(&format!("there is no setting {name}")))?;
        if !definition.per_session {
            return Err(misuse(&format!("{} is the database's: a session does not set it for itself", definition.name)));
        }
        self.0.retain(|(k, _)| *k != key);
        if let Some(v) = value {
            self.0.push((key, accepted(&definition.name, &definition.kind, v)?));
        }
        Ok(())
    }
}

// Unit tests: the registry's kinds and scopes (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::builtin::{LOCK_WAITS, text};
    use super::*;
    use crate::value::Decimal;

    /// Verifies: [DESIGN] RULES.md D36: each built-in key is its setting's
    /// place, and each default is of its kind.
    #[test]
    fn builtin_keys_are_their_settings() {
        let s = Settings::default();
        let names = [
            (DEFAULT_ENGINE, "DEFAULT_ENGINE"),
            (UNIQUE_INDEX_METHOD, "UNIQUE_INDEX_METHOD"),
            (TRANSACTION_POLICY, "TRANSACTION_POLICY"),
            (LOCK_WAIT, "LOCK_WAIT"),
            (SESSION_END, "SESSION_END"),
            (OPTIMIZER_MAX_PASSES, "OPTIMIZER.MAX_PASSES"),
            (INDEX_DESCENT_COST, "PLANNER.INDEX_DESCENT_COST"),
            (SORT_COMPARE_COST, "PLANNER.SORT_COMPARE_COST"),
            (CATALOG_NAME, "CATALOG_NAME"),
            (MEMORY_BUDGET, "MEMORY_BUDGET"),
            (WORK_MEMORY, "WORK_MEMORY"),
            (SPILL_BATCH, "SPILL_BATCH"),
            (EQUAL_SELECTIVITY, "PLANNER.EQUAL_SELECTIVITY"),
            (RANGE_SELECTIVITY, "PLANNER.RANGE_SELECTIVITY"),
            (NULL_FRACTION, "PLANNER.NULL_FRACTION"),
            (REORDER_GAIN, "PLANNER.REORDER_GAIN"),
            (DECORRELATE_AFTER, "EXECUTOR.DECORRELATE_AFTER"),
            (SPILL_PARTITIONS, "SPILL.PARTITIONS"),
            (SPILL_PARTITION_DEPTH, "SPILL.PARTITION_DEPTH"),
            (CHECKPOINT_AFTER_CHANGES, "CHECKPOINT.AFTER_CHANGES"),
            (CHECKPOINT_AFTER_BYTES, "CHECKPOINT.AFTER_BYTES"),
            (CATALOG_COMPACT_AFTER, "CATALOG.COMPACT_AFTER"),
            (PAGE_SIZE, "STORAGE.PAGE_SIZE"),
            (CHECKSUM, "STORAGE.CHECKSUM"),
            (ENCODINGS, "STORAGE.ENCODINGS"),
            (LOG_SEGMENT_SIZE, "LOG.SEGMENT_SIZE"),
            (POOL_BYTES, "STORAGE.POOL_BYTES"),
            (STATEMENT_TIMEOUT, "STATEMENT_TIMEOUT"),
            (LOCK_TIMEOUT, "LOCK_TIMEOUT"),
            (SLOW_STATEMENT, "DIAGNOSTICS.SLOW_STATEMENT"),
        ];
        assert_eq!(s.list(&Own::default()).len(), names.len());
        for (key, name) in names {
            assert_eq!(s.key(name), Some(key), "{name}");
            let d = s.definition(key).unwrap();
            assert_eq!(accepted(&d.name, &d.kind, &d.default).ok().map(|v| v.to_literal()), Some(d.default.to_literal()), "{name}");
        }
    }

    /// Verifies: [DESIGN] RULES.md D36, D42: a text setting takes text up
    /// to its length, its ends trimmed, an empty one too; not longer text,
    /// nor a number.
    #[test]
    fn text_is_taken_up_to_its_length() {
        let shown = |r: Result<Value>| r.ok().map(|v| v.to_literal());
        let path = Kind::Text { max: 4 };
        assert_eq!(shown(accepted("X", &path, &text(" a/b "))), Some("'a/b'".to_owned()));
        assert_eq!(shown(accepted("X", &path, &text(""))), Some("''".to_owned()), "empty");
        assert!(accepted("X", &path, &text("a/b/c")).is_err(), "too long");
        assert!(accepted("X", &path, &whole(1)).is_err(), "not text");
    }

    /// Verifies: [DESIGN] RULES.md D36: a value is taken only of its
    /// setting's kind and range: a whole number as one, a number of either
    /// kind, a word in any case; a value set before its setting is declared
    /// is its value once it is; a session sets only a setting of its own.
    #[test]
    fn values_are_taken_of_their_kind() {
        let range = Kind::Integer { min: 1, max: 10 };
        let shown = |r: Result<Value>| r.ok().map(|v| v.to_literal());
        assert_eq!(shown(accepted("X", &range, &Value::Exact(Decimal::new(500, 2)))), Some("5".to_owned()));
        for refused in [Value::Exact(Decimal::new(55, 1)), whole(11), whole(0), text("5")] {
            assert!(accepted("X", &range, &refused).is_err(), "{refused}");
        }
        let number = Kind::Number { min: 0.0_f64, max: 1.0_f64 };
        assert_eq!(shown(accepted("X", &number, &Value::Exact(Decimal::new(5_i128, 1_u32)))), Some(Value::Approx(0.5).to_literal()));
        assert!(accepted("X", &number, &Value::Approx(f64::NAN)).is_err());
        assert_eq!(shown(accepted("X", &Kind::Choice(LOCK_WAITS), &text(" wait "))), Some("'WAIT'".to_owned()));
        assert!(accepted("X", &Kind::Choice(LOCK_WAITS), &text("SOMETIMES")).is_err());

        let mut s = Settings::default();
        s.set("plugin.depth", Some(&whole(3))).unwrap();
        let declared = Definition {
            name: "Plugin.Depth".into(),
            kind: range,
            default: whole(1),
            per_session: false,
            effect: Effect::Statement,
            doc: String::new(),
        };
        let key = s.declare(declared.clone()).unwrap();
        assert_eq!((s.integer(key), s.effective(key, &Own::default()).1), (3, Source::Database));
        assert!(s.declare(declared).is_err(), "twice");
        s.set("PLUGIN.DEPTH", None).unwrap();
        assert_eq!((s.integer(key), s.effective(key, &Own::default()).1), (1, Source::Default));

        let mut own = Own::default();
        own.set(&s, "lock_wait", Some(&text("wait"))).unwrap();
        assert_eq!(lock_wait(s.effective(LOCK_WAIT, &own).0), LockWait::Wait);
        assert!(own.set(&s, "PLUGIN.DEPTH", Some(&whole(2))).is_err(), "the database's");
        assert!(own.set(&s, "NO.SUCH", None).is_err());
        own.set(&s, "LOCK_WAIT", None).unwrap();
        assert_eq!(s.effective(LOCK_WAIT, &own).1, Source::Default);
    }
}
