//! \[EXTENSION\] A plug-in's authentication mechanism (RULES.md D169).
//! The server's `Authenticator` is the server's own trait, so the server
//! wraps this one (`server/src/plugins.rs`): BEGIN starts an exchange on a
//! process kept for it, and each STEP answers the client's bytes with a
//! challenge, an acceptance or a refusal. The process is let go as the
//! exchange is dropped.

use std::sync::Arc;

use crate::bytes::Put;
use crate::error::Result;
use crate::plugin::errors::violation;

use crate::plugin::payload::{AUTH_ACCEPT, AUTH_BEGIN, AUTH_CHALLENGE, AUTH_REFUSE, AUTH_STEP};
use crate::plugin::pool::{CallContext, Lease, Request};

use super::{Link, answer};

/// A plug-in's mechanism.
#[derive(Debug)]
pub struct PluginAuthenticator {
    link: Arc<Link>,
}

/// Who connects, as BEGIN tells the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connecting<'a> {
    /// The user named.
    pub user: &'a str,
    /// The peer's address, as the server shows it.
    pub peer: &'a str,
    /// Whether the peer is on this machine.
    pub local: bool,
    /// Whether the connection is encrypted.
    pub secure: bool,
}

/// What a step of an exchange gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStep {
    /// Bytes for the client, whose answer is the next step's.
    Challenge(Vec<u8>),
    /// The user is who they say.
    Accept,
    /// The user is refused, and why.
    Refuse(String),
}

impl PluginAuthenticator {
    /// The mechanism `d` declares, named by its string.
    pub fn new(link: Link) -> PluginAuthenticator {
        PluginAuthenticator { link: Arc::new(link) }
    }

    /// The mechanism's name.
    pub fn mechanism(&self) -> &str {
        &self.link.object
    }

    /// Begins an exchange with `who`, on a process kept for it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T05 if no process is free; the program's own error.
    pub fn begin(&self, who: &Connecting<'_>) -> Result<PluginExchange> {
        let mut lease = self.link.pool.lease()?;
        let mut payload = Vec::new();
        payload.put_str(who.user);
        payload.put_str(who.peer);
        payload.put_u8(u8::from(who.local));
        payload.put_u8(u8::from(who.secure));
        let got = call(&mut lease, &self.link, AUTH_BEGIN, payload)?;
        let exchange = answer(&got, super::number)?;
        Ok(PluginExchange { link: Arc::clone(&self.link), lease, exchange })
    }
}

/// An exchange begun, on its own process.
pub struct PluginExchange {
    link: Arc<Link>,
    lease: Lease,
    exchange: u64,
}

impl std::fmt::Debug for PluginExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PluginExchange({:?}, {})", self.link, self.exchange)
    }
}

impl PluginExchange {
    /// The next step, of the client's `bytes`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T01 if its process ended; 38T03 of an outcome no step's.
    pub fn step(&mut self, bytes: &[u8]) -> Result<AuthStep> {
        let mut payload = Vec::new();
        payload.put_u64(self.exchange);
        payload.put_bytes(bytes);
        let got = call(&mut self.lease, &self.link, AUTH_STEP, payload)?;
        answer(&got, |r| match r.u8()? {
            AUTH_CHALLENGE => Ok(AuthStep::Challenge(r.bytes()?.to_vec())),
            AUTH_ACCEPT => Ok(AuthStep::Accept),
            AUTH_REFUSE => Ok(AuthStep::Refuse(r.str()?)),
            other => Err(violation(&format!("no step has the outcome {other}"))),
        })
    }
}

/// Operation `op` of `link`'s object, on the process `lease` keeps.
fn call(lease: &mut Lease, link: &Link, op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
    lease.call(&Request { kind: link.kind, op, object: &link.object, payload }, &mut CallContext { interrupt: None, sql: None })
}
