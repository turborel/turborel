//! \[EXTENSION\] A plug-in served (RULES.md D167): HELLO answered by
//! WELCOME, each CALL by RESULT or ERROR, until SHUTDOWN or the end of
//! its input. A call is given a `Cx`, through which it hears a CANCEL,
//! runs SQL in the call's transaction, and reads the settings its
//! manifest declares and reads, as the last SETTINGS before it gave them
//! (RULES.md D282).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::mpsc::Receiver;

use turborel::plugin::wire::{BATCH, CANCEL_CAPABILITY, Call, Hello, Message, SETTINGS_CAPABILITY, SQL_CAPABILITY, Welcome, write_message};
use turborel::value::Value;

use super::registry::Plugin;
use super::{Answer, PluginError, dispatch};

/// The limits HELLO gave (RULES.md D170).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    /// The most bytes of one frame either side sends.
    pub max_message_bytes: u64,
    /// The most bytes a call's answer is, all frames together.
    pub max_call_bytes: u64,
    /// The memory the program is to stay within.
    pub memory_bytes: u64,
}

/// What a call reaches the database by.
pub(super) trait Link {
    /// Whether the database asked to cancel call `call`.
    fn cancelled(&mut self, call: u64) -> bool;
    /// The rows of `text` run in call `call`'s transaction with `args`.
    ///
    /// # Errors
    ///
    /// The database's.
    fn sql(&mut self, call: u64, rows: bool, text: &str, args: &[Value]) -> Answer<Vec<Vec<Value>>>;
}

/// What a call is given.
pub struct Cx<'a> {
    pub(super) call: u64,
    pub(super) link: &'a mut dyn Link,
    pub(super) limits: Limits,
    pub(super) settings: &'a [(String, Value)],
}

impl Cx<'_> {
    /// Whether the database asked to cancel this call: a call that runs
    /// long asks, and stops.
    pub fn cancelled(&mut self) -> bool {
        self.link.cancelled(self.call)
    }

    /// That the call is not cancelled.
    ///
    /// # Errors
    ///
    /// SQLSTATE 57014 if it is.
    pub fn check(&mut self) -> Answer<()> {
        if self.cancelled() { Err(PluginError::new("57014", "the call was cancelled")) } else { Ok(()) }
    }

    /// The rows of the query `text`, with `args` for its `?`s, in the
    /// call's transaction, by its user's privileges.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn query(&mut self, text: &str, args: &[Value]) -> Answer<Vec<Vec<Value>>> {
        self.link.sql(self.call, true, text, args)
    }

    /// Runs the statement `text`, with `args` for its `?`s, as `query`.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn run(&mut self, text: &str, args: &[Value]) -> Answer<()> {
        self.link.sql(self.call, false, text, args).map(drop)
    }

    /// The limits HELLO gave.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The value the call runs with of the setting `name`: one its
    /// manifest declares, named under its extension's name
    /// (`EXAMPLE.DEPTH`), or a built-in one its READS SETTINGS names; of a
    /// session's call, the session's own value if it set one. None of a
    /// setting it neither declares nor reads (RULES.md D282).
    pub fn setting(&self, name: &str) -> Option<&Value> {
        self.settings.iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, v)| v)
    }

    /// The whole number of the setting `name`, if it is one.
    pub fn integer(&self, name: &str) -> Option<i128> {
        let Value::Exact(d) = self.setting(name)? else { return None };
        (d.scale == 0).then_some(d.units)
    }

    /// The word or text of the setting `name`, if it is one.
    pub fn text(&self, name: &str) -> Option<&str> {
        let Value::Char(s) = self.setting(name)? else { return None };
        Some(s)
    }

    /// Every setting the call runs with, each with its name.
    pub fn settings(&self) -> &[(String, Value)] {
        self.settings
    }
}

/// The database's side of a served plug-in: its messages, and what was
/// read while a call ran that is not for it.
struct Pipes<W: Write> {
    from: Receiver<Message>,
    to: W,
    backlog: VecDeque<Message>,
    cancelled: Vec<u64>,
}

impl<W: Write> Pipes<W> {
    /// The next message, none at the end.
    fn next(&mut self) -> Option<Message> {
        self.backlog.pop_front().or_else(|| self.from.recv().ok())
    }

    /// Writes `m`.
    fn send(&mut self, m: &Message) -> std::io::Result<()> {
        write_message(&mut self.to, m).map_err(|e| std::io::Error::other(e.msg))
    }
}

impl<W: Write> Link for Pipes<W> {
    fn cancelled(&mut self, call: u64) -> bool {
        while let Ok(m) = self.from.try_recv() {
            if let Message::Cancel { call: asked } = m {
                self.cancelled.push(asked);
            } else {
                self.backlog.push_back(m);
            }
        }
        self.cancelled.contains(&call)
    }
    fn sql(&mut self, call: u64, rows: bool, text: &str, args: &[Value]) -> Answer<Vec<Vec<Value>>> {
        let asked = Message::Sql { call, rows, text: text.to_owned(), args: args.to_vec() };
        self.send(&asked).map_err(|e| PluginError::external(&e.to_string()))?;
        loop {
            match self.from.recv() {
                Ok(Message::SqlDone { rows: given, .. }) => return Ok(given),
                Ok(Message::Error { state, message, .. }) => return Err(PluginError { state, message }),
                Ok(Message::Cancel { call: other }) => self.cancelled.push(other),
                Ok(other) => self.backlog.push_back(other),
                Err(_) => return Err(PluginError::external("the database closed the pipe")),
            }
        }
    }
}

/// Serves `plugin` on the process's standard input and output, until the
/// database shuts it down.
///
/// # Errors
///
/// If a pipe fails, or the thread reading the input cannot be started.
pub fn serve(plugin: Plugin) -> std::io::Result<()> {
    serve_on(plugin, std::io::stdin(), std::io::stdout())
}

/// Serves `plugin`, reading `input` and writing `output`.
///
/// # Errors
///
/// As `serve`.
pub fn serve_on(mut plugin: Plugin, input: impl Read + Send + 'static, output: impl Write) -> std::io::Result<()> {
    let from = super::input::messages(input)?;
    let mut pipes = Pipes { from, to: output, backlog: VecDeque::new(), cancelled: Vec::new() };
    let mut limits = Limits::default();
    while let Some(m) = pipes.next() {
        match m {
            Message::Hello(h) => {
                limits = Limits { max_message_bytes: h.max_message_bytes, max_call_bytes: h.max_call_bytes, memory_bytes: h.memory_bytes };
                pipes.send(&welcome(&plugin, &h))?;
            }
            Message::Call(c) => {
                pipes.cancelled.clear();
                let answer = called(&mut plugin, &mut pipes, limits, &c);
                pipes.send(&answer)?;
            }
            Message::Settings(told) => plugin.settings = told,
            Message::Shutdown => return Ok(()),
            Message::Welcome(_)
            | Message::Result { .. }
            | Message::Error { .. }
            | Message::Cancel { .. }
            | Message::Sql { .. }
            | Message::SqlDone { .. } => {}
        }
    }
    Ok(())
}

/// WELCOME of `hello`: protocol 1, what it takes of what the host offers,
/// and the kinds `plugin` serves.
pub(super) fn welcome(plugin: &Plugin, hello: &Hello) -> Message {
    let taken = [BATCH, SQL_CAPABILITY, CANCEL_CAPABILITY, SETTINGS_CAPABILITY];
    let capabilities = hello.capabilities.iter().filter(|c| taken.contains(&c.as_str())).cloned().collect();
    Message::Welcome(Welcome {
        protocol: 1,
        name: hello.extension.clone(),
        version: plugin.version.clone(),
        capabilities,
        kinds: plugin.kinds(),
    })
}

/// The answer of `c`, RESULT or ERROR, an answer longer than the limits
/// allow an ERROR of SQLSTATE 38T04, as the database would refuse it.
pub(super) fn called(plugin: &mut Plugin, link: &mut dyn Link, limits: Limits, c: &Call) -> Message {
    let told = std::mem::take(&mut plugin.settings);
    let mut cx = Cx { call: c.call, link, limits, settings: &told };
    let got = dispatch::answer(plugin, &mut cx, (c.kind, c.op, &c.object), &c.payload);
    plugin.settings = told;
    let most = [limits.max_message_bytes, limits.max_call_bytes].into_iter().filter(|m| *m > 0).min();
    let too_long = |p: &Vec<u8>| most.is_some_and(|m| u64::try_from(p.len()).unwrap_or(u64::MAX).saturating_add(16) > m);
    let refused = |state: String, message: String| Message::Error { call: c.call, state, message };
    match got {
        Ok(payload) => {
            if too_long(&payload) {
                refused("38T04".to_owned(), "the answer is longer than the database takes".to_owned())
            } else {
                Message::Result { call: c.call, payload }
            }
        }
        Err(e) => refused(e.state, e.message),
    }
}
