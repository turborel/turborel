//! \[EXTENSION\] A plug-in's backup target (RULES.md D169): a
//! `TargetProvider` of the scheme its manifest names, whose targets are
//! the program's. Opening a target keeps a process for it, as its uploads
//! live in that program, and lets it go as the target is dropped.

use std::sync::{Arc, Mutex};

use crate::bytes::Put;
use crate::error::Result;
use crate::plugin::payload::{
    TARGET_ABANDON, TARGET_BEGIN, TARGET_FINISH, TARGET_LIST, TARGET_OPEN, TARGET_READ, TARGET_REMOVE, TARGET_WRITE, strings,
};
use crate::plugin::pool::{CallContext, Lease, Request};
use crate::sync::lock_builtin;

use super::{Link, answer};
use crate::backup::{BackupTarget, TargetProvider};

/// A plug-in's scheme of backup targets.
#[derive(Debug)]
pub struct PluginTargets {
    link: Arc<Link>,
    /// \[EXTENSION\] The node's stop, of a host plug-in's archive (RULES.md
    /// D274), which a process failing trips.
    stop: Option<super::archive::Stopping>,
}

impl PluginTargets {
    /// The scheme `link` names.
    pub fn new(link: Link) -> PluginTargets {
        PluginTargets { link: Arc::new(link), stop: None }
    }

    /// \[EXTENSION\] The scheme `link` names, of a host plug-in whose
    /// process failing trips `stop` (RULES.md D274).
    pub fn stopping(link: Link, stop: super::archive::Stopping) -> PluginTargets {
        PluginTargets { link: Arc::new(link), stop: Some(stop) }
    }
}

impl TargetProvider for PluginTargets {
    fn scheme(&self) -> &str {
        &self.link.object
    }
    fn open(&self, location: &str) -> Result<Arc<dyn BackupTarget>> {
        let mut lease = self.link.pool.lease()?;
        let mut payload = Vec::new();
        payload.put_str(location);
        let got = call(&mut lease, (&self.link, self.stop.as_ref()), TARGET_OPEN, payload)?;
        let target = answer(&got, super::number)?;
        let (link, stop) = (Arc::clone(&self.link), self.stop.clone());
        Ok(Arc::new(PluginTarget { link, stop, location: location.to_owned(), target, lease: Mutex::new(lease) }))
    }
}

/// A target opened, on a process of its own.
struct PluginTarget {
    link: Arc<Link>,
    stop: Option<super::archive::Stopping>,
    location: String,
    target: u64,
    lease: Mutex<Lease>,
}

impl PluginTarget {
    /// Operation `op` of the target, its id first in `payload`'s.
    fn ask(&self, op: u8, rest: impl FnOnce(&mut Vec<u8>)) -> Result<Vec<u8>> {
        let mut payload = Vec::new();
        payload.put_u64(self.target);
        rest(&mut payload);
        call(&mut lock_builtin(&self.lease), (&self.link, self.stop.as_ref()), op, payload)
    }

    /// Operation `op` of an upload, answered by nothing.
    fn upload(&self, op: u8, upload: u64, bytes: Option<&[u8]>) -> Result<()> {
        let got = self.ask(op, |p| {
            p.put_u64(upload);
            if let Some(bytes) = bytes {
                p.put_bytes(bytes);
            }
        })?;
        answer(&got, |_| Ok(()))
    }
}

impl BackupTarget for PluginTarget {
    fn location(&self) -> String {
        self.location.clone()
    }
    fn begin(&self, name: &str) -> Result<u64> {
        let got = self.ask(TARGET_BEGIN, |p| p.put_str(name))?;
        answer(&got, super::number)
    }
    fn write(&self, upload: u64, bytes: &[u8]) -> Result<()> {
        self.upload(TARGET_WRITE, upload, Some(bytes))
    }
    fn finish(&self, upload: u64) -> Result<()> {
        self.upload(TARGET_FINISH, upload, None)
    }
    fn abandon(&self, upload: u64) -> Result<()> {
        self.upload(TARGET_ABANDON, upload, None)
    }
    fn read(&self, name: &str, offset: u64, max: u32) -> Result<Vec<u8>> {
        let got = self.ask(TARGET_READ, |p| {
            p.put_str(name);
            p.put_u64(offset);
            p.put_u32(max);
        })?;
        answer(&got, |r| r.bytes().map(<[u8]>::to_vec))
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let got = self.ask(TARGET_LIST, |p| p.put_str(prefix))?;
        answer(&got, strings)
    }
    fn remove(&self, name: &str) -> Result<()> {
        let got = self.ask(TARGET_REMOVE, |p| p.put_str(name))?;
        answer(&got, |_| Ok(()))
    }
}

/// Operation `op` of `link`'s object, on the process `lease` keeps; its
/// process failing trips `stop`, if given.
fn call(lease: &mut Lease, (link, stop): (&Link, Option<&super::archive::Stopping>), op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
    let got = lease.call(&Request { kind: link.kind, op, object: &link.object, payload }, &mut CallContext { interrupt: None, sql: None });
    match stop {
        Some(stop) => got.map_err(|e| stop.failed(link, e)),
        None => got,
    }
}
