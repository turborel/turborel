//! \[EXTENSION\] A transport served by a host plug-in (kind 32, RULES.md
//! D271): each frame sent one SEND, the frames arrived taken by RECEIVE
//! as the driver turns (`Driver::pump_with`), the members those OPEN and
//! RECONFIGURE answered, kept, so `members` makes no call. A frame is a
//! field of a whole message of protocol 1, which the host reads whole or
//! refuses, so a program that ends mid-message gives no part of one.

use std::sync::{Arc, Mutex, PoisonError};

use turborel::bytes::Put;
use turborel::error::{Code, Result, SqlError};
use turborel::plugin::payload::replication::{
    TRANSPORT_OPEN, TRANSPORT_RECEIVE, TRANSPORT_RECONFIGURE, TRANSPORT_SEND, arrived, members, put_configuration,
};

use super::convert::configuration_to;
use super::{Pipe, Stopping, byte};
use crate::driver::Inbox;
use crate::{Configuration, ReplicaId, Transport};

/// \[EXTENSION\] A host plug-in's transport.
pub struct PluginTransport {
    pipe: Stopping,
    members: Mutex<Vec<ReplicaId>>,
}

impl PluginTransport {
    /// The transport of replica `me` of `configuration` over `pipe`,
    /// opened: OPEN links the members.
    ///
    /// # Errors
    ///
    /// The pipe's.
    pub fn open(pipe: Box<dyn Pipe>, me: ReplicaId, configuration: &Configuration) -> Result<PluginTransport> {
        let pipe = Stopping::new(pipe, "replica transport");
        let mut payload = Vec::new();
        payload.put_u32(me.0);
        put_configuration(&mut payload, &configuration_to(configuration));
        let linked = pipe.ask(TRANSPORT_OPEN, payload, members)?;
        Ok(PluginTransport { pipe, members: Mutex::new(linked.into_iter().map(ReplicaId).collect()) })
    }

    /// \[EXTENSION\] The frames arrived since last asked, each from its
    /// replica, whole.
    ///
    /// # Errors
    ///
    /// The pipe's: the replica stops.
    pub fn receive(&self) -> Result<Vec<(ReplicaId, Vec<u8>)>> {
        let got = self.pipe.ask(TRANSPORT_RECEIVE, Vec::new(), arrived)?;
        Ok(got.into_iter().map(|(from, frame)| (ReplicaId(from), frame)).collect())
    }

    /// \[EXTENSION\] What the driver's turns call first, as its pump: the
    /// frames arrived put in `inbox`.
    pub fn pump(self: &Arc<Self>, inbox: Arc<Inbox>) -> Arc<dyn Fn() -> Result<()> + Send + Sync> {
        let me = Arc::clone(self);
        Arc::new(move || {
            for (from, frame) in me.receive()? {
                inbox.push(from, frame);
            }
            Ok(())
        })
    }

    /// Its failure, if it failed.
    pub fn failed(&self) -> Option<SqlError> {
        self.pipe.failed()
    }
}

impl Transport for PluginTransport {
    fn send(&self, to: ReplicaId, frame: &[u8]) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_u32(to.0);
        payload.put_bytes(frame);
        let sent = self.pipe.ask(TRANSPORT_SEND, payload, byte)?;
        if sent == 0 {
            return Err(SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D57: replica {} is no member the transport links", to.0)));
        }
        Ok(())
    }

    fn members(&self) -> Vec<ReplicaId> {
        self.members.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn reconfigure(&self, configuration: &Configuration) -> Result<()> {
        let mut payload = Vec::new();
        put_configuration(&mut payload, &configuration_to(configuration));
        let linked = self.pipe.ask(TRANSPORT_RECONFIGURE, payload, members)?;
        *self.members.lock().unwrap_or_else(PoisonError::into_inner) = linked.into_iter().map(ReplicaId).collect();
        Ok(())
    }
}
