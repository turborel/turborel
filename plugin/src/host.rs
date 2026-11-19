//! \[EXTENSION\] The host plug-ins' objects (RULES.md D229): the base each
//! host kind's trait is served through. A host object answers each
//! operation of its kind from its payload's bytes; each kind's module
//! gives a trait of its own and serves it so, its payloads read and
//! written as the database's `plugin::payload` gives them.

pub mod replication;

use super::registry::Plugin;
use super::serve::Cx;
use super::{Answer, PluginError};

/// An object of a host kind, as the protocol calls it: an operation and
/// its payload, answered with the RESULT's payload.
pub trait HostObject: Send {
    /// The answer of operation `op` with `payload`.
    ///
    /// # Errors
    ///
    /// The object's, sent as an ERROR, which stops the node as a host
    /// plug-in's failure does unless the kind takes it as an answer.
    fn answer(&mut self, cx: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>>;
}

impl Plugin {
    /// It, serving `object` as the object `name` of the host kind of code
    /// `kind` (`turborel::plugin::manifest::HostKind::code`).
    #[must_use]
    pub fn host(mut self, kind: u8, name: &str, object: impl HostObject + 'static) -> Plugin {
        self.hosted.insert((kind, name.to_owned()), Box::new(object));
        self
    }
}

/// The answer of the host object `object` of kind `kind`.
///
/// # Errors
///
/// The object's; SQLSTATE 38000 if there is no such object.
pub(super) fn answer(plugin: &mut Plugin, cx: &mut Cx<'_>, (kind, op, object): (u8, u8, &str), payload: &[u8]) -> Answer<Vec<u8>> {
    let found = plugin.hosted.get_mut(&(kind, object.to_owned()));
    let host = found.ok_or_else(|| PluginError::external(&format!("the plug-in has no host object {object} of kind {kind}")))?;
    host.answer(cx, op, payload)
}
