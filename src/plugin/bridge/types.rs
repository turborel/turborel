//! \[EXTENSION\] A plug-in's type (RULES.md D223), a `SqlType` whose
//! values the program makes and the database keeps and works with alone.
//! A value is made of its text form (INPUT), as a character string is
//! cast or assigned to the type, or of its binary form (RECEIVE), as a
//! binary string is cast to it; the program answers its three forms
//! (`payload/types.rs`), and nothing after that is a call: comparing,
//! hashing, keying, storing, showing and logging a value read its key,
//! binary and text bytes.
//!
//! So a value outlives its program: one kept is read, compared and shown
//! with the extension's processes stopped, or the extension gone. A type
//! is the same kind (`Kind`) in every state of the database that has its
//! extension, kept once per database (`type_of`), as a column's type holds
//! its kind by a `'static` reference; it makes its values through the
//! newest of that database's pools of its extension still running
//! (`attach`), never another database's in the same process.
//!
//! What a plug-in type cannot do: take parameters, be cast to or from
//! any type but a character or binary string and itself, or order its
//! values but by its keys as bytes. If its manifest says not ORDERED, its
//! values are neither ordered, grouped nor keyed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};
use crate::plugin::errors::violation;
use crate::plugin::manifest::ObjectKind;
use crate::plugin::payload::types::{Made, answered, made, put_binaries, put_made, put_texts};
use crate::plugin::payload::{TYPE_INPUT, TYPE_RECEIVE};
use crate::plugin::pool::Pool;
use crate::sync::lock_builtin;
use crate::value::types::{ExtensionType, ExtensionValue, Kind, TypeParams};
use crate::value::{DataType, SqlType, Value};

use super::{Link, answer, counted, leaked};

/// A plug-in's type.
pub struct PluginType {
    /// The database it is of (`Host::owner`), 0 for none.
    owner: u64,
    name: &'static str,
    extension: String,
    ordered: bool,
    /// Its extension's pools, newest last, held weakly.
    pools: Mutex<Vec<Weak<Pool>>>,
}

impl std::fmt::Debug for PluginType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.extension, self.name)
    }
}

/// The types made, by database, extension, name and order, each with its
/// kind.
type Kinds = BTreeMap<(u64, String, String, bool), (&'static PluginType, Kind)>;

/// The type `name` of `extension`, ORDERED or not, of the database
/// `owner` (`Host::owner`), and its kind: the same each time that
/// database asks, and another for each other database in the process, so
/// its values are made by that database's own processes.
pub fn type_of(owner: u64, extension: &str, name: &str, ordered: bool) -> (&'static PluginType, Kind) {
    static KINDS: OnceLock<Mutex<Kinds>> = OnceLock::new();
    let mut kinds = lock_builtin(KINDS.get_or_init(Mutex::default));
    let key = (owner, extension.to_owned(), name.to_owned(), ordered);
    if let Some(found) = kinds.get(&key) {
        return *found;
    }
    let made: &'static PluginType =
        Box::leak(Box::new(PluginType { owner, name: leaked(name), extension: extension.to_owned(), ordered, pools: Mutex::default() }));
    let as_type: &'static dyn SqlType = made;
    let kind: Kind = Box::leak(Box::new(as_type));
    kinds.insert(key, (made, kind));
    (made, kind)
}

/// \[EXTENSION\] The types the extension `manifest` declares added to
/// `types`, as a catalog is read before its extensions are made (RULES.md
/// D223): a type's values are read with no process.
///
/// # Errors
///
/// SQLSTATE 38T06 if the manifest is not one; SQLCODE -910 if another
/// type of a name it declares is registered.
pub fn declare(manifest: &str, types: &mut crate::value::types::TypeRegistry) -> Result<()> {
    let manifest = crate::plugin::Manifest::parse(manifest)?;
    for d in manifest.objects.iter().filter(|d| d.kind == ObjectKind::Type) {
        types.add_kind(type_of(types.owner(), &manifest.name, &d.name, d.ordered()).1)?;
    }
    Ok(())
}

/// That `kind`, a plug-in's, makes its values through `pool` from now on.
pub fn attach(kind: &PluginType, pool: &Arc<Pool>) {
    let mut pools = lock_builtin(&kind.pools);
    pools.retain(|p| p.strong_count() > 0);
    pools.push(Arc::downgrade(pool));
}

/// The three forms of a value's bytes, if they are a value's.
fn forms(bytes: &[u8]) -> Option<Made> {
    let mut r = Reader::new(bytes, "a plug-in value");
    let m = made(&mut r).ok()?;
    r.finish().ok().map(|()| m)
}

impl PluginType {
    /// Its kind, as the process keeps it.
    pub fn kind(&self) -> Kind {
        type_of(self.owner, &self.extension, self.name, self.ordered).1
    }

    /// Its type.
    pub fn data_type(&self) -> DataType {
        DataType::Extension(ExtensionType { kind: self.kind(), params: TypeParams::default() })
    }

    /// Its name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Whether `kind` is it.
    pub fn is(&self, kind: &dyn SqlType) -> bool {
        std::ptr::addr_eq(std::ptr::from_ref(kind), std::ptr::from_ref::<dyn SqlType>(*self.kind()))
    }

    /// The way to its program, through its newest pool running.
    fn link(&self) -> Result<Link> {
        let pools = lock_builtin(&self.pools);
        let pool = pools.iter().rev().find_map(Weak::upgrade).ok_or_else(|| {
            let why = format!("[EXTENSION] no value of the type {} is made: its extension {} is not created", self.name, self.extension);
            SqlError::new(Code::Api, why)
        })?;
        Ok(Link { pool, kind: ObjectKind::Type.code(), object: self.name.to_owned() })
    }

    /// The value of `m`.
    fn value_of(&self, m: &Made) -> Value {
        let mut bytes = Vec::new();
        put_made(&mut bytes, m);
        Value::Extension(ExtensionValue { kind: *self.kind(), bytes: bytes.into() })
    }

    /// The forms of `v`, if a value of this type.
    fn forms_of(&self, v: &Value) -> Option<Made> {
        let Value::Extension(e) = v else { return None };
        (e.kind.name() == self.name).then(|| forms(&e.bytes)).flatten()
    }

    /// The value the program makes of `text` (INPUT) or `binary` (RECEIVE).
    fn made(&self, op: u8, payload: Vec<u8>) -> Result<Value> {
        let got = answer(&self.link()?.plain(op, payload)?, answered)?;
        let m = counted(got, 1)?.pop().ok_or_else(|| violation("it made no value"))?;
        Ok(self.value_of(&m))
    }

    /// The value of the character string `text`.
    ///
    /// # Errors
    ///
    /// The program's, or none running.
    pub fn input(&self, text: &str) -> Result<Value> {
        let mut payload = Vec::new();
        put_texts(&mut payload, &[text]);
        self.made(TYPE_INPUT, payload)
    }

    /// The value of the binary form `bytes`.
    ///
    /// # Errors
    ///
    /// As `input`.
    pub fn receive(&self, bytes: &[u8]) -> Result<Value> {
        let mut payload = Vec::new();
        put_binaries(&mut payload, &[bytes]);
        self.made(TYPE_RECEIVE, payload)
    }

    /// The error of `v`, not a value of it.
    fn mismatch(&self, v: &Value) -> SqlError {
        SqlError::new(Code::Api, format!("[DESIGN] {v} is not a {} value", self.name))
    }
}

mod kind;

#[cfg(test)]
mod tests;
