//! \[DESIGN\] Settings (RULES.md D36): the registry of every setting the
//! database has, each declared once, with its kind, its default, whether a
//! session may set it for itself, and when a change takes effect; and the
//! values set, the database's and a session's own.
//!
//! The built-in settings are declared here; a plug-in declares its own
//! (`Configurable`). A database setting is a definition, logged and
//! replayed with the others (RULES.md D25), unless it is a node's
//! (`Definition::node`, D57), never logged nor in an image; one replayed
//! before the plug-in that declares it is kept until then. A session's end
//! with it. A name is an upper case `<identifier>`, parts joined by periods.

mod builtin;
mod configurable;
mod kind;
mod started;
mod words;

use std::sync::Arc;

use crate::error::{Code, Result, SqlError};
use crate::txn::Policy;
use crate::value::{Value, pow10};

pub use builtin::{
    CATALOG_COMPACT_AFTER_IMAGES, CATALOG_NAME, CHECKPOINT_AFTER_BYTES, CHECKPOINT_AFTER_CHANGES, CHECKPOINT_OVERDUE_FACTOR, CHECKSUM,
    COMMIT_SYNC, DECORRELATE_AFTER_RUNS, DIAGNOSTICS_LEVEL, ENCODINGS, EQUAL_SELECTIVITY, EXECUTOR_BATCHES, FILES_MAX_OPEN, FUNCTION_CALLS,
    INDEX_DESCENT_COST, JOBS_POLICY, LOCKS_TIMEOUT_MS, LOCKS_WAIT, LOG_SEGMENT_BYTES, LOG_SYNC_AFTER_MS, MEMORY_BUDGET_BYTES,
    MEMORY_WORK_BYTES, NULL_FRACTION, OFF, ON, OPTIMIZER_MAX_PASSES, PAGE_BYTES, POOL_BYTES, RANGE_SELECTIVITY, READ_AHEAD_PAGES,
    REORDER_GAIN, SESSION_COMMIT_AT_END, SESSION_TIME_ZONE, SLOW_STATEMENT_MS, SORT_COMPARE_COST, SPILL_BATCH_BYTES, SPILL_ENCODINGS,
    SPILL_PARTITION_DEPTH, SPILL_PARTITIONS, STATEMENT_TIMEOUT_MS, SWITCHES, TABLES_ENGINE, TABLES_UNIQUE_INDEX_METHOD, TRANSACTION_STRICT,
};
use builtin::{builtin, whole};
pub use configurable::Configurable;
pub use kind::{Kind, column_list, column_names};
pub use started::Started;
use words::word_of;
pub use words::{lock_wait, lock_wait_word, log_durability, policy_word, session_end, session_end_word};

/// When a change of a setting takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// For every statement that begins after it.
    Statement,
    /// For every transaction that begins after it.
    Transaction,
    /// For every connection a server takes after it, read as it is taken.
    Connection,
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
    /// \[DESIGN\] Whether it is the node's (RULES.md D36, D57): its value,
    /// never logged nor written to an image, which the replicas of a
    /// database share, is given as the node starts, and lasts while it
    /// runs. Neither a session's nor fixed at creation.
    pub node: bool,
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
    /// \[DESIGN\] The node's, of a node's setting (`Definition::node`): set
    /// as the node started or since, kept nowhere.
    Node,
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
    /// Of one taken as what reads it starts (`Effect::Start`), the value
    /// it was started with, if it started: a value apart from it waits
    /// for a restart.
    pub started: Option<Value>,
}

/// The bytes a value of MEMORY.BUDGET_BYTES says.
pub fn budget_bytes(value: &Value) -> usize {
    bytes(value, crate::memory::DEFAULT_BUDGET)
}

/// The bytes a value of STORAGE.POOL_BYTES says.
pub fn pool_bytes(value: &Value) -> usize {
    bytes(value, crate::db::POOL_BYTES)
}

/// The bytes a value of MEMORY.WORK_BYTES says.
pub fn work_bytes(value: &Value) -> usize {
    bytes(value, crate::spill::DEFAULT_WORK_MEMORY)
}

/// The bytes a value of SPILL.BATCH_BYTES says.
pub fn batch_bytes(value: &Value) -> usize {
    bytes(value, crate::spill::DEFAULT_SPILL_BATCH)
}

/// The bytes a whole number `value` says, or `default` if it is not one.
fn bytes(value: &Value, default: usize) -> usize {
    match value {
        Value::Exact(d) => usize::try_from(d.units).unwrap_or(usize::MAX),
        Value::Approx(_)
        | Value::Char(_)
        | Value::Null
        | Value::Bits(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Extension(_) => default,
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

/// `value` as a value of `kind`, for the setting `name`: a whole number as
/// an exact number of scale 0, a number as an approximate one, a word upper
/// case, or a name, names or text as a character string, its ends trimmed.
///
/// # Errors
///
/// SQLCODE -910 if it is not of the kind, or out of its range.
pub(crate) fn accepted(name: &str, kind: &Kind, value: &Value) -> Result<Value> {
    let refused = || misuse(&format!("{name} takes {}, not {}", kind.described(), value.to_literal()));
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
        // Each a column of the table: `stored` checks them.
        Kind::Columns { most } => {
            let names = given.and_then(column_names).filter(|n| n.len() <= *most).ok_or_else(refused)?;
            Ok(Value::Char(column_list(&names)))
        }
        Kind::Identifier => {
            let body = given.map(str::trim).filter(|s| !s.is_empty() && s.chars().count() <= crate::lexer::MAX_IDENTIFIER_LENGTH);
            body.map(|s| Value::Char(s.to_owned())).ok_or_else(refused)
        }
        Kind::Displacement => given
            .map(str::trim)
            .and_then(crate::value::datetime::parse_displacement)
            .map(|z| Value::Char(crate::value::Datetime::displacement_text(z)))
            .ok_or_else(refused),
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
    /// The values the settings read as what reads them starts were
    /// started with.
    started: Arc<Started>,
}

impl Default for Settings {
    fn default() -> Self {
        let definitions = builtin();
        let values = vec![None; definitions.len()];
        Settings { definitions: Arc::new(definitions), values, pending: Vec::new(), started: Arc::default() }
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
        if definition.node && (definition.per_session || definition.effect == Effect::Creation) {
            return Err(misuse(&format!("{} is a node's: neither a session's nor fixed at creation (RULES.md D36)", definition.name)));
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
    /// settings declared, then those set before theirs were. A node's are
    /// not the database's: an image never holds them (RULES.md D57).
    pub(crate) fn database_values(&self) -> Vec<(String, Value)> {
        let kept = self.definitions.iter().zip(&self.values).filter(|(d, _)| !d.node);
        let set = kept.filter_map(|(d, v)| Some((d.name.clone(), v.clone()?)));
        set.chain(self.pending.iter().cloned()).collect()
    }

    /// The value of `key` with a session's `own` values over the
    /// database's, and where it comes from.
    pub fn effective<'a>(&'a self, key: Key, own: &'a Own) -> (&'a Value, Source) {
        if let Some(v) = own.get(key) {
            return (v, Source::Session);
        }
        let set = self.values.get(key.0).is_some_and(Option::is_some);
        let node = self.definition(key).is_some_and(|d| d.node);
        let source = match (set, node) {
            (false, _) => Source::Default,
            (true, false) => Source::Database,
            (true, true) => Source::Node,
        };
        (self.get(key), source)
    }

    /// Every setting, with its value as a session with `own` values sees it.
    pub fn list(&self, own: &Own) -> Vec<Listed> {
        (0..self.definitions.len())
            .filter_map(|k| {
                let key = Key(k);
                let (value, source) = self.effective(key, own);
                let definition = self.definition(key)?;
                let started = (definition.effect == Effect::Start).then(|| self.started.get(&definition.name)).flatten();
                Some(Listed { definition: definition.clone(), value: value.clone(), source, started })
            })
            .collect()
    }

    /// Records the database's value of `key` as the one what reads it was
    /// started with, or `value` if given: what it took, if not what is set.
    pub fn mark_started(&self, key: Key, value: Option<Value>) {
        if let Some(d) = self.definition(key) {
            self.started.record(&d.name, value.unwrap_or_else(|| self.get(key).clone()));
        }
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

    /// Whether `key`, a setting of `SWITCHES`, is ON.
    pub fn is_on(&self, key: Key) -> bool {
        self.word(key) == ON
    }

    /// [X3.135-1986 §4.16] The transaction policy.
    pub fn policy(&self) -> Policy {
        if self.word(TRANSACTION_STRICT) == OFF { Policy::Permissive } else { Policy::Strict }
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

    /// Whether it sets the values `other` sets, of the same settings.
    pub fn same_as(&self, other: &Own) -> bool {
        self.0.len() == other.0.len() && self.0.iter().all(|(k, v)| other.get(*k).is_some_and(|w| w.to_literal() == v.to_literal()))
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
mod tests;
