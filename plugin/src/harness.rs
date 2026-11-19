//! \[EXTENSION\] A plug-in run in a test's own process (RULES.md
//! R-TEST-1): each call made as the database makes it, its payload
//! written and its answer read as protocol 1 carries them, with no
//! database and no process. SQL a call runs is answered by the test.

use turborel::plugin::payload::{self as p, collation, types};
use turborel::plugin::values::values;
use turborel::plugin::wire::{Call, Hello, Message, put_rows};
use turborel::value::Value;
use turborel::value::collation::Pad;

use super::registry::Plugin;
use super::serve::{Limits, Link, called, welcome};
use super::{Answer, Made, PluginError};

/// What answers a call's SQL in a test: the statement, its values, and
/// whether rows are wanted.
type Sql = Box<dyn FnMut(&str, &[Value], bool) -> Answer<Vec<Vec<Value>>>>;

/// A plug-in under test.
///
/// ```
/// use turborel_plugin::{Answer, Collation, Harness, Plugin};
///
/// struct Lower;
///
/// impl Collation for Lower {
///     fn sort_key(&mut self, text: &str, _pad: bool) -> Answer<Vec<u8>> {
///         Ok(text.to_lowercase().into_bytes())
///     }
/// }
///
/// let mut h = Harness::new(Plugin::new("1.0.0").collation("LOWER", Lower));
/// assert_eq!(h.kinds("EXAMPLE"), [12]);
/// assert_eq!(h.sort_keys("LOWER", &["B", "a"], false).unwrap(), [b"b".to_vec(), b"a".to_vec()]);
/// ```
pub struct Harness {
    plugin: Plugin,
    sql: Sql,
    cancel: bool,
    next: u64,
    limits: Limits,
}

/// The link of a test: its SQL answered by the test, a call cancelled if
/// asked.
struct TestLink<'a> {
    sql: &'a mut Sql,
    cancel: bool,
}

impl Link for TestLink<'_> {
    fn cancelled(&mut self, _call: u64) -> bool {
        self.cancel
    }
    fn sql(&mut self, _call: u64, rows: bool, text: &str, args: &[Value]) -> Answer<Vec<Vec<Value>>> {
        (self.sql)(text, args, rows)
    }
}

impl Harness {
    /// `plugin` under test, its SQL refused.
    pub fn new(plugin: Plugin) -> Harness {
        let limits = Limits { max_message_bytes: 1 << 24, max_call_bytes: 1 << 26, memory_bytes: 1 << 28 };
        Harness { plugin, sql: Box::new(|_, _, _| Err(PluginError::external("no SQL in this test"))), cancel: false, next: 0, limits }
    }

    /// Its calls' SQL answered by `sql`.
    #[must_use]
    pub fn with_sql(mut self, sql: impl FnMut(&str, &[Value], bool) -> Answer<Vec<Vec<Value>>> + 'static) -> Harness {
        self.sql = Box::new(sql);
        self
    }

    /// Its calls answered within `limits`, as HELLO would give them.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Harness {
        self.limits = limits;
        self
    }

    /// Its calls made with the settings `told`, each with its name, as a
    /// SETTINGS frame gives them (RULES.md D282).
    pub fn settings(&mut self, told: &[(&str, Value)]) {
        self.plugin.settings = told.iter().map(|(n, v)| ((*n).to_owned(), v.clone())).collect();
    }

    /// Its calls cancelled, as the database asks, or not.
    pub fn cancel(&mut self, cancel: bool) {
        self.cancel = cancel;
    }

    /// The kinds WELCOME gives, of a HELLO of extension `name`.
    pub fn kinds(&self, name: &str) -> Vec<u8> {
        let hello = Hello {
            protocol: 1,
            extension: name.to_owned(),
            capabilities: Vec::new(),
            max_message_bytes: 0,
            max_call_bytes: 0,
            memory_bytes: 0,
        };
        match welcome(&self.plugin, &hello) {
            Message::Welcome(w) => w.kinds,
            Message::Hello(_)
            | Message::Call(_)
            | Message::Result { .. }
            | Message::Error { .. }
            | Message::Cancel { .. }
            | Message::Shutdown
            | Message::Sql { .. }
            | Message::Settings(_)
            | Message::SqlDone { .. } => Vec::new(),
        }
    }

    /// The RESULT payload of operation `op` of the object `object` of kind
    /// `kind`, given `payload`, as the database calls it.
    ///
    /// # Errors
    ///
    /// The ERROR the plug-in answers.
    pub fn call(&mut self, kind: u8, op: u8, object: &str, payload: Vec<u8>) -> Answer<Vec<u8>> {
        self.next = self.next.saturating_add(1);
        let c = Call { call: self.next, kind, op, object: object.to_owned(), payload };
        let mut link = TestLink { sql: &mut self.sql, cancel: self.cancel };
        match called(&mut self.plugin, &mut link, self.limits, &c) {
            Message::Result { payload: answered, .. } => Ok(answered),
            Message::Error { state, message, .. } => Err(PluginError { state, message }),
            Message::Hello(_)
            | Message::Welcome(_)
            | Message::Call(_)
            | Message::Cancel { .. }
            | Message::Shutdown
            | Message::Sql { .. }
            | Message::Settings(_)
            | Message::SqlDone { .. } => Err(PluginError::external("no answer")),
        }
    }

    /// The values the scalar function `object` gives of `rows`, one a row.
    ///
    /// # Errors
    ///
    /// The function's.
    pub fn scalar(&mut self, object: &str, rows: &[Vec<Value>]) -> Answer<Vec<Value>> {
        let mut payload = Vec::new();
        put_rows(&mut payload, rows)?;
        let got = self.call(1, p::SCALAR_CALL, object, payload)?;
        Ok(values(&mut p::reader(&got))?)
    }

    /// The sort keys the collation `object` gives `texts`.
    ///
    /// # Errors
    ///
    /// The collation's.
    pub fn sort_keys(&mut self, object: &str, texts: &[&str], pad: bool) -> Answer<Vec<Vec<u8>>> {
        let mut payload = Vec::new();
        collation::put_keys(&mut payload, if pad { Pad::Space } else { Pad::None }, texts);
        let got = self.call(12, p::COLLATION_KEYS, object, payload)?;
        Ok(collation::answered(&mut p::reader(&got))?)
    }

    /// The value the type `object` makes of `text`.
    ///
    /// # Errors
    ///
    /// The type's.
    pub fn input(&mut self, object: &str, text: &str) -> Answer<Made> {
        let mut payload = Vec::new();
        types::put_texts(&mut payload, &[text]);
        let got = self.call(15, p::TYPE_INPUT, object, payload)?;
        types::answered(&mut p::reader(&got))?.pop().ok_or_else(|| PluginError::external("no value made"))
    }
}
