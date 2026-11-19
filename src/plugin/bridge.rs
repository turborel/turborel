//! \[EXTENSION\] The objects of an extension, each bridged onto the trait
//! the database already has for its kind (RULES.md D169): a call of one is
//! a CALL to the extension's pool, its values in protocol 1.
//!
//! An extension installed (`Installed`) holds its manifest, its pool and
//! its objects. The database's state holds it, so a state dropped, as a
//! transaction rolled back drops its own, drops the extension's pool and
//! stops its processes.
//!
//! * `scalar.rs`, `aggregate.rs`, `table.rs`, `procedure.rs`: functions
//!   and procedures; `metric.rs`, `embedder.rs`: vectors.
//! * `auth.rs`: an authenticator, which the server wraps.
//! * `backup.rs`, `partition.rs`: backup targets and partitioning
//!   strategies.
//! * `ranking.rs`, `quantizer.rs`, `collation.rs`: full-text rankings,
//!   vector quantizers and collations (RULES.md D221).
//! * `system.rs`, `xquery.rs`, `process.rs`: system tables, XQuery
//!   functions, credentials and diagnostics (RULES.md D222).
//! * `types.rs`, `engine.rs`, `index.rs`: types, storage engines and
//!   index methods (RULES.md D223 to D227).
//!
//! `kinds.rs` holds the object of each kind, bridged.

pub mod aggregate;
pub mod archive;
pub mod auth;
pub mod backup;
pub mod collation;
pub mod embedder;
pub mod engine;
pub mod hosted;
pub mod index;
mod kinds;
pub mod metric;
pub mod partition;
pub mod procedure;
pub mod process;
pub mod quantizer;
pub mod ranking;
pub mod scalar;
pub mod system;
pub mod table;
pub mod types;
pub mod xquery;

pub use kinds::Bridged;
use kinds::bridge;

use std::path::PathBuf;
use std::sync::Arc;

use crate::ast::TypeName;
use crate::bytes::Reader;
use crate::error::Result;
use crate::value::DataType;

use super::errors::{invalid_manifest, violation};
use super::host::Host;
use super::manifest::{Declared, Manifest, ObjectKind};
use super::pool::{CallContext, Pool, Request};

/// What resolves a type a manifest names, as a column's is.
pub type Resolve<'a> = &'a dyn Fn(&TypeName) -> Result<DataType>;

/// An extension installed: its manifest, directory, pool and objects.
pub struct Installed {
    /// Its manifest.
    pub manifest: Manifest,
    /// Its directory, absolute.
    pub directory: PathBuf,
    /// Its processes.
    pub pool: Arc<Pool>,
    /// Its objects, in the manifest's order.
    pub objects: Vec<(Declared, Bridged)>,
    /// The key that signed its package, if one did.
    pub signer: Option<String>,
}

impl std::fmt::Debug for Installed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Installed").field("name", &self.manifest.name).field("directory", &self.directory).finish_non_exhaustive()
    }
}

impl Installed {
    /// The extension `manifest` declares, in `directory`, its objects
    /// bridged, their types resolved by `resolve`; no process is started
    /// until a call needs one.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if a declaration names a type `resolve` refuses, or
    /// one no value of protocol 1 carries; 38T05 if no executable is for
    /// this platform.
    pub fn new(manifest: Manifest, directory: PathBuf, host: &Arc<Host>, resolve: Resolve<'_>) -> Result<Installed> {
        let executable = manifest.executable(std::env::consts::OS)?;
        let limits = (Arc::clone(&host.limits), Arc::clone(&host.memory));
        let pool = Pool::new(&manifest.name, (directory.clone(), executable), manifest.kinds(), limits).with_check(host.check());
        let pool = pool.with_feed(super::settings::Feed::of(&manifest, false));
        let pool = Arc::new(pool);
        let objects = manifest
            .objects
            .iter()
            .map(|d| Ok((d.clone(), bridge(d, &pool, resolve).map_err(|e| declared_wrongly(d, &e))?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Installed { manifest, directory, pool, objects, signer: None })
    }

    /// Its objects of `kind`.
    pub fn of_kind(&self, kind: ObjectKind) -> impl Iterator<Item = &Bridged> {
        self.objects.iter().filter(move |(d, _)| d.kind == kind).map(|(_, b)| b)
    }
}

/// `e`, a declaration's fault, as 38T06 naming it.
fn declared_wrongly(d: &Declared, e: &crate::error::SqlError) -> crate::error::SqlError {
    invalid_manifest(&format!("{} {}: {}", d.kind.word(), d.name, e.msg))
}

/// `name`, kept as long as the program runs, as the traits whose names
/// are `&'static str` take them (a type, a system table); each name is
/// kept once, however often an extension is made again.
pub fn leaked(name: &str) -> &'static str {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};
    static KEPT: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut kept = crate::sync::lock_builtin(KEPT.get_or_init(Mutex::default));
    if let Some(found) = kept.get(name) {
        return found;
    }
    let made: &'static str = Box::leak(name.to_owned().into_boxed_str());
    kept.insert(made);
    made
}

/// The types `names` resolve to.
fn resolved(names: &[TypeName], resolve: Resolve<'_>) -> Result<Vec<DataType>> {
    names.iter().map(|t| checked_type(resolve(t)?)).collect()
}

/// `t`, if protocol 1 carries its values: no structured type or reference.
fn checked_type(t: DataType) -> Result<DataType> {
    let carried = !matches!(t, DataType::Structured(_) | DataType::Reference(_));
    if carried { Ok(t) } else { Err(invalid_manifest(&format!("no value of {t} is carried by protocol 1"))) }
}

/// The way to an object of an extension: its pool, kind and name.
#[derive(Clone)]
pub struct Link {
    /// The pool.
    pub pool: Arc<Pool>,
    /// Its kind's code.
    pub kind: u8,
    /// Its name.
    pub object: String,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.pool.extension(), self.object)
    }
}

impl Link {
    /// Calls operation `op` with `payload`, in `cx`: its RESULT's payload.
    ///
    /// # Errors
    ///
    /// The program's error, or the pool's (`Pool::call`).
    pub fn call(&self, op: u8, payload: Vec<u8>, cx: &mut CallContext<'_>) -> Result<Vec<u8>> {
        self.pool.call(&Request { kind: self.kind, op, object: &self.object, payload }, cx)
    }

    /// Calls operation `op` with `payload`, uncancellable, calling no SQL.
    ///
    /// # Errors
    ///
    /// As `call`.
    pub fn plain(&self, op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
        self.call(op, payload, &mut CallContext { interrupt: None, sql: None })
    }

    /// What its extension's calls are named, in EXPLAIN.
    pub fn named(&self) -> String {
        format!("{}.{}", self.pool.extension(), self.object)
    }
}

/// `read` of a RESULT's payload, which must be read whole.
///
/// # Errors
///
/// SQLSTATE 38T03 if it does not hold what `read` reads, or more.
pub fn answer<T>(payload: &[u8], read: impl FnOnce(&mut Reader<'_>) -> Result<T>) -> Result<T> {
    let mut r = super::payload::reader(payload);
    let made = read(&mut r).and_then(|v| r.finish().map(|()| v));
    made.map_err(|e| violation(&format!("its RESULT is malformed: {}", e.msg)))
}

/// That `got` values answered `wanted` asked.
///
/// # Errors
///
/// SQLSTATE 38T03 if the counts differ.
pub fn counted<T>(got: Vec<T>, wanted: usize) -> Result<Vec<T>> {
    if got.len() == wanted { Ok(got) } else { Err(violation(&format!("it answered {} values for {wanted}", got.len()))) }
}

/// A RESULT's number: an exchange, a target or an upload.
///
/// # Errors
///
/// `Reader`'s, if it holds none.
pub fn number(r: &mut Reader<'_>) -> Result<u64> {
    r.u64()
}

/// A RESULT's byte: a truth or an outcome.
///
/// # Errors
///
/// `Reader`'s, if it holds none.
pub fn byte(r: &mut Reader<'_>) -> Result<u8> {
    r.u8()
}
