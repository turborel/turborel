//! \[DESIGN\] What a database gives its extensions, shared by every state
//! of it (RULES.md D170-D173): the limits its settings say, the memory
//! budget frames are reserved from, the keys it trusts, and the
//! cryptography a package's digests and signature are checked by, which
//! the host registers, as the core makes no cryptography of its own (D26).
//!
//! A plug-in installed from a package is checked as each process starts,
//! so a file changed after it was installed stops it from running
//! (SQLSTATE 38T07).

use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use crate::error::Result;
use crate::memory::MemoryBudget;

use super::errors::unavailable;
use super::hosted::{HostPlugin, Stop};
use super::limits::{Limits, Told};
use super::package::{PackageCrypto, Trust, verify_installed};
use super::payload::statistics::Asked;

/// What checks a plug-in's directory as a process of it starts.
pub type Check = Arc<dyn Fn(&Path) -> Result<Option<String>> + Send + Sync>;

/// What answers the statistics a plug-in asks (RULES.md D279): the
/// database's newest, with the table and column they are of, for the
/// privileges of a call that runs SQL to be checked.
pub type Statistician = Arc<dyn Fn(&Asked) -> Result<Answered> + Send + Sync>;

/// The statistics a plug-in asked for, written, and what they are of.
#[derive(Debug, Clone)]
pub struct Answered {
    /// The table's schema and name.
    pub table: (String, String),
    /// The column, if one was asked of.
    pub column: Option<String>,
    /// The statistics, as STATISTICSDONE carries them.
    pub payload: Vec<u8>,
}

/// What a database gives its extensions.
pub struct Host {
    /// The limits of PLUGINS.*.
    pub limits: Arc<Limits>,
    /// The budget a program's frames are reserved from.
    pub memory: Arc<MemoryBudget>,
    crypto: RwLock<Option<Arc<dyn PackageCrypto>>>,
    trust: RwLock<Trust>,
    /// The node's stop, which a host plug-in's failure trips (RULES.md D229).
    pub stop: Arc<Stop>,
    hosted: RwLock<Vec<Arc<HostPlugin>>>,
    /// What the data file's pool made next evicts by: CLOCK, or a host
    /// plug-in's BUFFER EVICTION's evictor (RULES.md D283).
    pub eviction: crate::disk::buffer::EvictionSlot,
    statistician: RwLock<Option<Statistician>>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host").field("limits", &self.limits).finish_non_exhaustive()
    }
}

impl Host {
    /// A host of the budget `memory`, its limits the defaults, trusting
    /// no key, no cryptography registered.
    pub fn new(memory: Arc<MemoryBudget>) -> Host {
        Host {
            limits: Arc::new(Limits::default()),
            memory,
            crypto: RwLock::new(None),
            trust: RwLock::new(Trust::default()),
            stop: Stop::new(),
            hosted: RwLock::new(Vec::new()),
            eviction: crate::disk::buffer::EvictionSlot::default(),
            statistician: RwLock::new(None),
        }
    }

    /// Registers what answers the statistics plug-ins ask, as the database
    /// opens.
    pub fn set_statistician(&self, statistician: Statistician) {
        *self.statistician.write().unwrap_or_else(PoisonError::into_inner) = Some(statistician);
    }

    /// The statistics a plug-in asks, as a pool answers them.
    pub fn statistics(self: &Arc<Self>) -> Statistician {
        let host = Arc::clone(self);
        Arc::new(move |asked: &Asked| {
            let held = host.statistician.read().unwrap_or_else(PoisonError::into_inner).clone();
            held.ok_or_else(|| unavailable("the database gives no statistics yet"))?(asked)
        })
    }

    /// Keeps the host plug-in `plugin`, started (RULES.md D229).
    pub fn add_hosted(&self, plugin: Arc<HostPlugin>) {
        self.hosted.write().unwrap_or_else(PoisonError::into_inner).push(plugin);
    }

    /// The host plug-ins started, in the order they were.
    pub fn hosted(&self) -> Vec<Arc<HostPlugin>> {
        self.hosted.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Registers the cryptography packages are checked by.
    pub fn set_crypto(&self, crypto: Arc<dyn PackageCrypto>) {
        *self.crypto.write().unwrap_or_else(PoisonError::into_inner) = Some(crypto);
    }

    /// The cryptography registered.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T05 if none is.
    pub fn crypto(&self) -> Result<Arc<dyn PackageCrypto>> {
        let held = self.crypto.read().unwrap_or_else(PoisonError::into_inner).clone();
        held.ok_or_else(|| unavailable("no package cryptography is registered, which the host gives (Database::set_package_crypto)"))
    }

    /// Takes the settings' values now.
    pub fn tell(&self, told: &Told, trust: Trust) {
        self.limits.tell(told);
        *self.trust.write().unwrap_or_else(PoisonError::into_inner) = trust;
    }

    /// The keys trusted, and whether a signature is required.
    pub fn trust(&self) -> Trust {
        self.trust.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Checks the plug-in in `dir`, installed from a package or not: its
    /// signer, if a trusted key signed it.
    ///
    /// # Errors
    ///
    /// As `verify_installed`; SQLSTATE 38T05 if it was installed from a
    /// package and no cryptography is registered to check it.
    pub fn verify(&self, dir: &Path) -> Result<Option<String>> {
        let trust = self.trust();
        let packaged = dir.join(super::package::DIGESTS_FILE).exists();
        if !packaged && !trust.require_signed {
            return Ok(None);
        }
        verify_installed(dir, &*self.crypto()?, &trust)
    }

    /// `verify`, as a pool checks each process it starts.
    pub fn check(self: &Arc<Self>) -> Check {
        let host = Arc::clone(self);
        Arc::new(move |dir: &Path| host.verify(dir))
    }
}
