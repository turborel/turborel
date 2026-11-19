//! \[EXTENSION\] Host plug-ins (RULES.md D229): programs serving the
//! points beneath a database, its encodings, checksums, clock, log,
//! catalog storage, spill, file system, keys, package cryptography, jobs
//! and served connections, over plug-in protocol 1 as an extension's
//! objects are served.
//!
//! A host plug-in is declared by the node's configuration, the setting
//! PLUGINS.HOST the host gives and SQL may not set, never by CREATE
//! EXTENSION, so nothing of the catalog is needed to load one. Each is
//! started, a process of it running, before the database opens. A host
//! plug-in is fail-stop. A call of it that fails stops the node (`Stop`),
//! every later statement failing with SQLSTATE 38T11, and the database is
//! recovered as it next opens, so nothing a failed program half did is
//! taken as done.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::{Result, SqlError};

use super::MANIFEST_FILE;
use super::errors::{ENDED, TIMED_OUT, TOO_LONG, VIOLATION, host_failed, invalid_manifest, unavailable};
use super::host::Host;
use super::manifest::{HostDeclared, HostKind, Manifest};
use super::package::hex;
use super::pool::{CallContext, Lease, Pool, Request};
use crate::sync::lock_builtin;

/// The separator of PLUGINS.HOST's entries.
const ENTRIES: char = ';';
/// What follows an entry's directory when its program's digest is pinned.
const PINNED: char = '@';
/// The hexadecimal digits of a SHA-256 digest.
const DIGEST_HEX: usize = 64;

/// A host plug-in the node declares: its name, directory and, if pinned,
/// its program's SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// The extension's name, as its manifest gives it.
    pub name: String,
    /// Its directory.
    pub directory: PathBuf,
    /// Its program's SHA-256 digest, lower-case hexadecimal, if pinned.
    pub digest: Option<String>,
}

impl HostSpec {
    /// The host plug-ins of PLUGINS.HOST, `name=directory[@digest]`
    /// separated by `;`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if an entry has no name or directory, or a pinned
    /// digest is not 64 hexadecimal digits.
    pub fn parse_all(text: &str) -> Result<Vec<HostSpec>> {
        text.split(ENTRIES).map(str::trim).filter(|e| !e.is_empty()).map(HostSpec::parse).collect()
    }

    /// One entry.
    fn parse(entry: &str) -> Result<HostSpec> {
        let wrong = || invalid_manifest(&format!("PLUGINS.HOST entry {entry} is not name=directory[@sha256]"));
        let (name, rest) = entry.split_once('=').ok_or_else(wrong)?;
        let pinned = rest.rsplit_once(PINNED).filter(|(_, d)| d.len() == DIGEST_HEX && d.chars().all(|c| c.is_ascii_hexdigit()));
        let (directory, digest) = pinned.map_or((rest, None), |(dir, d)| (dir, Some(d.to_ascii_lowercase())));
        if name.trim().is_empty() || directory.trim().is_empty() {
            return Err(wrong());
        }
        Ok(HostSpec { name: name.trim().to_ascii_uppercase(), directory: PathBuf::from(directory.trim()), digest })
    }
}

/// What a told stop calls: the reason.
pub type Told = Arc<dyn Fn(&str) + Send + Sync>;

/// \[DESIGN\] The node's stop, which a host plug-in's failure trips once:
/// why it stopped, and who to tell.
#[derive(Default)]
pub struct Stop {
    tripped: AtomicBool,
    why: Mutex<Option<String>>,
    told: Mutex<Vec<Told>>,
}

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stop").field("why", &self.stopped()).finish_non_exhaustive()
    }
}

impl Stop {
    /// A stop not tripped.
    pub fn new() -> Arc<Stop> {
        Arc::new(Stop::default())
    }

    /// Trips it for `why`, telling each who asked, once: a later reason
    /// is not kept.
    pub fn stop(&self, why: &str) {
        let first = {
            let mut held = self.why.lock().unwrap_or_else(PoisonError::into_inner);
            let first = held.is_none();
            if first {
                *held = Some(why.to_owned());
                self.tripped.store(true, Ordering::Release);
            }
            first
        };
        if first {
            let told: Vec<Told> = self.told.lock().unwrap_or_else(PoisonError::into_inner).clone();
            for tell in told {
                tell(why);
            }
        }
    }

    /// Whether it stopped, read with no lock, as each statement asks.
    pub fn tripped(&self) -> bool {
        self.tripped.load(Ordering::Acquire)
    }

    /// Why it stopped, if it did.
    pub fn stopped(&self) -> Option<String> {
        self.why.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Asks that `tell` be called as it stops, at once if it has.
    pub fn on_stop(&self, tell: Told) {
        if let Some(why) = self.stopped() {
            tell(&why);
            return;
        }
        self.told.lock().unwrap_or_else(PoisonError::into_inner).push(tell);
    }
}

/// A host plug-in started: its manifest, directory and processes.
pub struct HostPlugin {
    /// Its manifest.
    pub manifest: Manifest,
    /// Its directory.
    pub directory: PathBuf,
    /// Its processes.
    pub pool: Arc<Pool>,
    /// The node's stop, which its failure trips.
    pub stop: Arc<Stop>,
    /// Whether PLUGINS.HOST pins its program's digest.
    pub pinned: bool,
}

impl std::fmt::Debug for HostPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostPlugin").field("name", &self.manifest.name).finish_non_exhaustive()
    }
}

impl HostPlugin {
    /// Starts the host plug-in `spec` names: its manifest read, its
    /// program checked against a pinned digest, and one process started.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if its manifest cannot be read, is not of `spec`'s
    /// name, or declares an extension's objects or no host object; 38T05
    /// if a digest is pinned and no cryptography is registered to check
    /// it, if a package cryptography plug-in's is not pinned, or if no
    /// process starts; 38T07 if the program is not of its pinned digest.
    pub fn start(spec: &HostSpec, host: &Arc<Host>, stop: &Arc<Stop>) -> Result<Arc<HostPlugin>> {
        let directory = spec.directory.canonicalize().map_err(|e| unavailable(&format!("{}: {e}", spec.directory.display())))?;
        let manifest = read_manifest(&directory, spec)?;
        let executable = manifest.executable(std::env::consts::OS)?;
        let program = directory.join(&executable);
        let pinned = pinned_check(host, spec, &program)?;
        pinned(&directory)?;
        let limits = (Arc::clone(&host.limits), Arc::clone(&host.memory));
        let pool = Pool::new(&manifest.name, (directory.clone(), executable), manifest.kinds(), limits).with_check(pinned).named_by_host();
        let pool = pool.with_feed(super::settings::Feed::of(&manifest, true));
        let plugin = Arc::new(HostPlugin { manifest, directory, pool: Arc::new(pool), stop: Arc::clone(stop), pinned: spec.digest.is_some() });
        drop(plugin.pool.lease()?);
        Ok(plugin)
    }

    /// Its objects of `kind`.
    pub fn declared(&self, kind: HostKind) -> impl Iterator<Item = &HostDeclared> {
        self.manifest.hosted.iter().filter(move |d| d.kind == kind)
    }

    /// The way to its object `d`.
    pub fn link(self: &Arc<Self>, d: &HostDeclared) -> HostLink {
        HostLink { plugin: Arc::clone(self), kind: d.kind.code(), object: d.name.clone() }
    }
}

/// The manifest in `directory`, of `spec`'s name, of host objects alone.
fn read_manifest(directory: &Path, spec: &HostSpec) -> Result<Manifest> {
    let text = std::fs::read_to_string(directory.join(MANIFEST_FILE)).map_err(|e| invalid_manifest(&format!("{MANIFEST_FILE}: {e}")))?;
    let manifest = Manifest::parse(&text)?;
    if !manifest.name.eq_ignore_ascii_case(&spec.name) {
        return Err(invalid_manifest(&format!("PLUGINS.HOST names {}, and its manifest {}", spec.name, manifest.name)));
    }
    if manifest.hosted.is_empty() {
        return Err(invalid_manifest(&format!(
            "{} declares no host object, so it is an extension, made by CREATE EXTENSION",
            manifest.name
        )));
    }
    Ok(manifest)
}

/// What checks the program as each process starts: its pinned digest, if
/// one is, and a package's digests and signature, as an extension's are.
fn pinned_check(host: &Arc<Host>, spec: &HostSpec, program: &Path) -> Result<super::host::Check> {
    let crypto_kind = Manifest::parse(&std::fs::read_to_string(spec.directory.join(MANIFEST_FILE)).unwrap_or_default())
        .is_ok_and(|m| m.hosted.iter().any(|d| d.kind == HostKind::PackageCrypto));
    if crypto_kind && spec.digest.is_none() {
        return Err(unavailable(&format!(
            "{} brings package cryptography, so PLUGINS.HOST must pin its program's digest, checked by the host's own (RULES.md D229)",
            spec.name
        )));
    }
    // The host's own cryptography as the plug-in starts, kept: a plug-in
    // that brings package cryptography replaces it, and is never checked
    // by itself.
    let own = spec.digest.as_ref().map(|_| host.crypto()).transpose()?;
    let (host, digest, program) = (Arc::clone(host), spec.digest.clone(), program.to_path_buf());
    Ok(Arc::new(move |dir: &Path| {
        if let (Some(wanted), Some(own)) = (&digest, &own) {
            let bytes = std::fs::read(&program).map_err(|e| unavailable(&format!("{}: {e}", program.display())))?;
            let got = hex(&own.sha256(&bytes));
            if &got != wanted {
                return Err(super::package::errors::changed(&format!(
                    "{} is of digest {got}, not the {wanted} PLUGINS.HOST pins",
                    program.display()
                )));
            }
        }
        host.verify(dir)
    }))
}

/// The way to a host plug-in's object: its plug-in, kind and name. A
/// failed call stops the node.
#[derive(Clone)]
pub struct HostLink {
    plugin: Arc<HostPlugin>,
    kind: u8,
    object: String,
}

impl std::fmt::Debug for HostLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.plugin.manifest.name, self.object)
    }
}

impl HostLink {
    /// Calls operation `op` with `payload`: its RESULT's payload. Any
    /// failure, the program's ERROR among them, stops the node.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the node is stopped, or stops of this call.
    pub fn call(&self, op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
        self.called(op, payload, |_| true)
    }

    /// Calls operation `op` with `payload`, as `call`, but a program's
    /// ERROR is the call's alone: only its process failing stops the node.
    ///
    /// # Errors
    ///
    /// The program's error; SQLSTATE 38T11 as `call`.
    pub fn ask(&self, op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
        self.called(op, payload, process_failed)
    }

    /// Its plug-in.
    pub fn plugin(&self) -> &Arc<HostPlugin> {
        &self.plugin
    }

    /// Its object's name.
    pub fn object(&self) -> &str {
        &self.object
    }

    /// Whether the node runs: an error once it has stopped.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if it has stopped.
    pub fn running(&self) -> Result<()> {
        match self.plugin.stop.stopped() {
            Some(why) => Err(host_failed(&self.plugin.manifest.name, &why)),
            None => Ok(()),
        }
    }

    /// One call, stopping the node if `stops` says its error should.
    fn called(&self, op: u8, payload: Vec<u8>, stops: fn(&SqlError) -> bool) -> Result<Vec<u8>> {
        self.running()?;
        let request = Request { kind: self.kind, op, object: &self.object, payload };
        self.failing(self.plugin.pool.call(&request, &mut CallContext::default()), stops)
    }

    /// `got`, its failure stopping the node if `stops` says it does.
    fn failing<T>(&self, got: Result<T>, stops: fn(&SqlError) -> bool) -> Result<T> {
        got.map_err(|e| {
            if !stops(&e) {
                return e;
            }
            let why = format!("{}.{}: {}", self.plugin.manifest.name, self.object, e.msg);
            self.plugin.stop.stop(&why);
            host_failed(&self.plugin.manifest.name, &e.msg)
        })
    }

    /// A process of its plug-in kept for this object's calls, as a spill
    /// area or a connection keeps what it holds in one program.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the node is stopped, or none can be leased.
    pub fn lease(&self) -> Result<HostLease> {
        self.running()?;
        let lease = self.failing(self.plugin.pool.lease(), |_| true)?;
        Ok(HostLease { link: self.clone(), lease: Mutex::new(lease) })
    }
}

/// A process kept for one host object's calls, each failure of which
/// stops the node.
pub struct HostLease {
    link: HostLink,
    lease: Mutex<Lease>,
}

impl std::fmt::Debug for HostLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} (kept)", self.link)
    }
}

impl HostLease {
    /// Calls operation `op` with `payload` on its process, as
    /// `HostLink::call`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the node is stopped, or stops of this call.
    pub fn call(&self, op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
        self.link.running()?;
        let request = Request { kind: self.link.kind, op, object: &self.link.object, payload };
        let got = lock_builtin(&self.lease).call(&request, &mut CallContext::default());
        self.link.failing(got, |_| true)
    }
}

/// Whether `e` is of a process failing, not of a program's ERROR.
fn process_failed(e: &SqlError) -> bool {
    e.state.is_some_and(|s| [ENDED, TIMED_OUT, VIOLATION, TOO_LONG].contains(&s))
}
