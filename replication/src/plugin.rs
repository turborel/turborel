//! \[EXTENSION\] Replication's extension points served by host plug-ins
//! (RULES.md D270 to D274): a transport (`PluginTransport`), a journal
//! (`PluginJournal`), a state transfer (`PluginTransfer`) and a
//! consensus (`PluginConsensus`), each a program of another party the
//! node's configuration declares (PLUGINS.HOST), its calls the payloads
//! of `turborel::plugin::payload::replication`.
//!
//! Each object is spoken to through a [`Pipe`], one process of its own
//! for the object's whole life ([`Hosted`]), as its state, a fetch, the
//! frames waiting, a consensus's view, lives in that process. A pipe is
//! fail-stop. Its first failure is kept and every later call fails with
//! it, no process is started again, and the driver, given the error,
//! halts and stops the node, so nothing a program half did is taken as
//! done. The replica is recovered from its journal as the node starts
//! again, as after a crash.
//!
//! The driver calls these with its lock held, as it calls every part
//! (RULES.md R-ERR-4's aim kept otherwise): a host plug-in's call makes no
//! SQL and so never calls back into the driver, and PLUGINS.CALL_TIMEOUT_MS
//! bounds it, past which the node stops.
//!
//! `node.rs` takes a node's host objects of these kinds.

mod consensus;
pub mod convert;
mod journal;
mod node;
mod transfer;
mod transport;

use std::sync::{Mutex, PoisonError};

use turborel::bytes::Reader;
use turborel::error::{Result, SqlError};
use turborel::plugin::bridge::answer;
use turborel::plugin::errors::host_failed;
use turborel::plugin::hosted::HostLink;
use turborel::plugin::pool::{CallContext, Lease, Request};

pub use self::consensus::PluginConsensus;
pub use self::journal::PluginJournal;
pub use self::node::{Plugged, plugged};
pub use self::transfer::PluginTransfer;
pub use self::transport::PluginTransport;

/// \[EXTENSION\] The way to one object of a plug-in: a call of an
/// operation with its payload, answered by its RESULT's payload.
pub trait Pipe: Send {
    /// Calls operation `op` with `payload`.
    ///
    /// # Errors
    ///
    /// The program's error, or its process failing.
    fn call(&mut self, op: u8, payload: Vec<u8>) -> Result<Vec<u8>>;
}

/// \[EXTENSION\] A host plug-in's object on a process of its own, leased
/// for the object's life: a failure stops the node (`Stop`), as any call
/// of a host plug-in that fails does.
pub struct Hosted {
    link: HostLink,
    kind: u8,
    lease: Lease,
}

impl Hosted {
    /// The object `link` names, of kind `kind`, a process leased for it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the node is stopped; the pool's, if no process
    /// is free or can be started.
    pub fn new(link: HostLink, kind: u8) -> Result<Hosted> {
        link.running()?;
        let lease = link.plugin().pool.lease()?;
        Ok(Hosted { link, kind, lease })
    }
}

impl Pipe for Hosted {
    fn call(&mut self, op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
        self.link.running()?;
        let request = Request { kind: self.kind, op, object: self.link.object(), payload };
        self.lease.call(&request, &mut CallContext::default()).map_err(|e| {
            let name = &self.link.plugin().manifest.name;
            self.link.plugin().stop.stop(&format!("{name}.{}: {}", self.link.object(), e.msg));
            host_failed(name, &e.msg)
        })
    }
}

/// A pipe that fails stop: its first failure kept, every later call
/// failing with it, no call made.
pub(crate) struct Stopping {
    pipe: Mutex<Box<dyn Pipe>>,
    failed: Mutex<Option<SqlError>>,
    /// What the object is, for messages.
    what: &'static str,
}

impl Stopping {
    /// `pipe`, to an object that is `what`.
    pub(crate) fn new(pipe: Box<dyn Pipe>, what: &'static str) -> Stopping {
        Stopping { pipe: Mutex::new(pipe), failed: Mutex::new(None), what }
    }

    /// Its failure, if it failed.
    pub(crate) fn failed(&self) -> Option<SqlError> {
        self.failed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Operation `op` with `payload`, its answer read whole by `read`;
    /// a failure, the answer's malformation among them, kept.
    pub(crate) fn ask<T>(&self, op: u8, payload: Vec<u8>, read: impl FnOnce(&mut Reader<'_>) -> Result<T>) -> Result<T> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let got = self.pipe.lock().unwrap_or_else(PoisonError::into_inner).call(op, payload);
        got.and_then(|bytes| answer(&bytes, read)).map_err(|e| self.fail(&e))
    }

    /// `e` kept as its failure, unless one was: the first is returned.
    pub(crate) fn fail(&self, e: &SqlError) -> SqlError {
        let mut failed = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        let kept = failed.get_or_insert_with(|| {
            let why = format!("[EXTENSION] RULES.md D270: the plug-in serving the {} failed, and the replica stops: {}", self.what, e.msg);
            let stopped = SqlError::new(e.code, why);
            match e.state {
                Some(state) => stopped.with_state(state),
                None => stopped,
            }
        });
        kept.clone()
    }
}

/// A RESULT's string.
pub(crate) fn text(r: &mut Reader<'_>) -> Result<String> {
    r.str()
}

/// A RESULT's byte.
pub(crate) fn byte(r: &mut Reader<'_>) -> Result<u8> {
    r.u8()
}

#[cfg(test)]
mod tests;
