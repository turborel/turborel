//! \[DESIGN\] What a connection asks of its session beyond statements, as
//! the server granted it (RULES.md D185): the session's settings as the
//! server last told them (D190), the notices it sent (D187), the log
//! position of the last commit (D194), a ping (D189), a reset or a change
//! of user (D189), the attributes each request carries (D191), and a
//! Query of several statements (D192).

use std::io::{Read, Write};

use turborel_protocol::capability::{CHANGE_USER, MULTI_STATEMENT, PING, QUERY_ATTRIBUTES, RESET};
use turborel_protocol::{Attributes, Capabilities, Failure, Notice, Reply, Request};

use crate::connection::{Connection, Outcome, Result, failed};
use crate::mechanism::Mechanism;
use crate::told::NoticeHandler;

/// What a Query of several statements gave: each statement's outcome, in
/// order, and the failure of the one that stopped it, if one did.
#[derive(Debug, Clone)]
pub struct Ran {
    /// The outcomes of the statements that ran, in order.
    pub outcomes: Vec<Outcome>,
    /// The failure of the statement that stopped the rest, if one did.
    pub failure: Option<Failure>,
}

impl<S: Read + Write> Connection<S> {
    /// What the server granted of what the connection asked for.
    pub fn granted(&self) -> Capabilities {
        self.told.granted
    }

    /// A connection failure, unless `ok` or the server granted `name`.
    pub(crate) fn needs(&self, ok: bool, name: &str) -> Result<()> {
        if ok || self.told.granted.has(name) {
            return Ok(());
        }
        Err(failed(&format!("[DESIGN] the server did not grant {name}")))
    }

    /// \[DESIGN\] RULES.md D190: the value of the session's setting `name`,
    /// as the server last told it, with no round trip: SESSION.TIME_ZONE,
    /// TRANSACTION.ISOLATION, any a session may set; AUTHORIZATION, SCHEMA,
    /// READ_ONLY and SERVER_VERSION. None if it told none of that name, as
    /// a server not granting `status` tells none.
    pub fn setting(&self, name: &str) -> Option<&str> {
        self.told.settings.get(name).map(String::as_str)
    }

    /// Every setting the server told, by name.
    pub fn settings(&self) -> impl Iterator<Item = (&str, &str)> {
        self.told.settings.iter().map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// \[DESIGN\] RULES.md D187: `handler` takes each notice the server
    /// sends from now on, as it comes, before the reply of the statement
    /// that raised it; none, they are kept (`take_notices`).
    pub fn set_notice_handler(&mut self, handler: Option<NoticeHandler>) {
        self.told.handler = handler;
    }

    /// The notices kept, with no handler set: taken.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.told.notices)
    }

    /// \[DESIGN\] RULES.md D194: the log position of the last commit the
    /// server told of, once granted `commit-position`; set it as
    /// REPLICATION.READ_AFTER on a replica's session, and that session's
    /// statements read what the commit made.
    pub fn commit_position(&self) -> Option<u64> {
        self.told.position
    }

    /// \[DESIGN\] RULES.md D197: the rows the last statement inserted,
    /// updated, deleted or merged, as GET DIAGNOSTICS ROW_COUNT gives
    /// them, or the rows the last query returned, once all were read; once
    /// granted `row-counts`.
    pub fn rows_affected(&self) -> Option<u64> {
        self.told.completed.as_ref()?.rows
    }

    /// \[DESIGN\] RULES.md D240: the server's time for the last statement,
    /// in microseconds, once granted `row-counts`: from when a worker began
    /// its request until its completion was ready, so neither the network
    /// nor the client is in it; a cursor's summed over its Execute and
    /// fetches; with the time its request waited for a worker, before the
    /// whole, and the time it waited for locks, among it.
    pub fn server_time(&self) -> Option<turborel_protocol::Timing> {
        self.told.completed.as_ref()?.time
    }

    /// \[DESIGN\] RULES.md D198: the identity values the last INSERT
    /// generated, in the order of its rows, as SQL writes them, once
    /// granted `generated-values`: none of another statement, or of an
    /// INSERT that gave the column its values, or of one whose table has no
    /// identity column. A trigger's statements' own are never among them.
    pub fn last_generated(&self) -> &[String] {
        self.told.completed.as_ref().map_or(&[], |c| c.generated.as_slice())
    }

    /// \[DESIGN\] RULES.md D187: each condition the last statement raised
    /// and completed with, once granted `conditions`: the warnings of
    /// class 01 and the conditions of class 02 its routines signalled, and
    /// the one it completed with.
    pub fn conditions(&self) -> &[Failure] {
        self.told.completed.as_ref().map_or(&[], |c| c.conditions.as_slice())
    }

    /// \[DESIGN\] RULES.md D191: `attributes` carried by each Query and
    /// Execute from now on, read in SQL by QUERY_ATTRIBUTE(name);
    /// `traceparent` is the request's trace context, which the server's
    /// record of a slow statement carries. None, for none.
    ///
    /// # Errors
    ///
    /// A connection failure if `query-attributes` was not granted, for
    /// attributes not none.
    pub fn set_query_attributes(&mut self, attributes: Attributes) -> Result<()> {
        self.needs(attributes.is_empty(), QUERY_ATTRIBUTES)?;
        self.told.attributes = attributes;
        Ok(())
    }

    /// \[DESIGN\] RULES.md D189: whether the server answers, with no worker
    /// of its taken.
    ///
    /// # Errors
    ///
    /// A connection failure: `ping` not granted, or the server gone.
    pub fn ping(&mut self) -> Result<()> {
        self.needs(false, PING)?;
        self.send(&Request::Ping)?;
        match self.receive()? {
            Reply::Pong => Ok(()),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Complete(_)
            | Reply::Notice(_)
            | Reply::Status { .. } => Err(self.lose("[DESIGN] the server sent no Pong")),
        }
    }

    /// \[DESIGN\] RULES.md D189: the session back to its state at login: its
    /// transaction rolled back, its prepared statements, cursors,
    /// temporary tables and settings gone, its user and cancel key kept.
    /// Statements prepared before are of no use after.
    ///
    /// # Errors
    ///
    /// A connection failure: `reset` not granted, or the server gone.
    pub fn reset(&mut self) -> Result<()> {
        self.needs(false, RESET)?;
        self.completed(&Request::Reset).map(drop)
    }

    /// \[DESIGN\] RULES.md D189: the connection authenticated again as
    /// `user` by `mechanism`, its session renewed as that user's, as
    /// `reset` renews it, under a new cancel key.
    ///
    /// # Errors
    ///
    /// The server's refusal, which closes the connection; a connection
    /// failure: `change-user` not granted, or the server gone.
    pub fn change_user(&mut self, user: &str, mechanism: &mut dyn Mechanism) -> Result<()> {
        self.needs(false, CHANGE_USER)?;
        let (name, initial) = (mechanism.name().to_owned(), mechanism.initial());
        self.send(&Request::ChangeUser { user: user.to_owned(), mechanism: name, initial })?;
        loop {
            match self.receive()? {
                Reply::Challenge(challenge) => {
                    let answer = mechanism.answer(&challenge)?;
                    self.send(&Request::Answer(answer))?;
                }
                Reply::Ready { authorization, key, capabilities: _, server } => {
                    mechanism.ready()?;
                    (self.authorization, self.server, self.key) = (authorization, server, Some(key));
                    return Ok(());
                }
                Reply::Error(failure) => return Err(self.failed(failure)),
                Reply::Prepared { .. }
                | Reply::Description(_)
                | Reply::Rows(_)
                | Reply::Suspended
                | Reply::Complete(_)
                | Reply::Lob(_)
                | Reply::Notice(_)
                | Reply::Status { .. }
                | Reply::Pong => return Err(self.lose("[DESIGN] the server sent no Ready")),
            }
        }
    }

    /// \[DESIGN\] RULES.md D192: runs `sql`, several statements, in one
    /// request: the server splits it as `Splitter` does and runs each in
    /// order, each the transaction ended with it under `set_autocommit`;
    /// the first that fails stops the rest.
    ///
    /// # Errors
    ///
    /// A connection failure: `multi-statement` not granted, or the server
    /// gone.
    pub fn query_many(&mut self, sql: &str) -> Result<Ran> {
        self.needs(false, MULTI_STATEMENT)?;
        let attributes = self.told.attributes();
        self.send(&Request::Query { sql: sql.to_owned(), ends: self.autocommit, attributes, multi: true })?;
        let mut ran = Ran { outcomes: Vec::new(), failure: None };
        loop {
            match self.outcome() {
                Ok((outcome, more)) => {
                    ran.outcomes.push(outcome);
                    if !more {
                        return Ok(ran);
                    }
                }
                Err(failure) => {
                    if self.is_lost() {
                        return Err(failure);
                    }
                    ran.failure = Some(failure);
                    return Ok(ran);
                }
            }
        }
    }

    /// One statement's outcome of a Query of several, and whether another
    /// statement's replies follow.
    fn outcome(&mut self) -> Result<(Outcome, bool)> {
        let columns = match self.receive()? {
            Reply::Complete(completed) => {
                let more = completed.more;
                return Ok((Outcome::Done(completed), more));
            }
            Reply::Description(columns) => columns,
            Reply::Error(failure) => return Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => return Err(self.lose("[DESIGN] the server sent no result")),
        };
        let mut rows = Vec::new();
        loop {
            match self.receive()? {
                Reply::Rows(batch) => rows.extend(batch),
                Reply::Complete(completed) => return Ok((Outcome::Rows { columns, rows }, completed.more)),
                Reply::Error(failure) => return Err(self.failed(failure)),
                Reply::Lob(_)
                | Reply::Challenge(_)
                | Reply::Ready { .. }
                | Reply::Prepared { .. }
                | Reply::Description(_)
                | Reply::Suspended
                | Reply::Notice(_)
                | Reply::Status { .. }
                | Reply::Pong => return Err(self.lose("[DESIGN] the server broke off its rows")),
            }
        }
    }
}
