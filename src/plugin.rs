//! \[EXTENSION\] Extensions from other parties, run as plug-in processes
//! (RULES.md D167-D173).
//!
//! An extension is a directory holding a manifest (`manifest.rs`) and a
//! program. The database starts the program on demand and talks to it
//! over its standard input and output in the plug-in protocol, version 1
//! (`wire.rs`), so a crash or hang of the program never touches the
//! database, and the program may be written in any language. Its
//! functions, aggregates, table functions, procedures, vector metrics,
//! embedding models and authenticators are bridged onto the traits the
//! database already has (`bridge.rs`), so they are called as a registered
//! plug-in of the same kind is.
//!
//! * `wire.rs`: frames and messages; `values.rs`: values in them;
//!   `payload.rs`: what each kind's calls carry.
//! * `manifest.rs`: the manifest, as `parser/manifest.rs` reads it.
//! * `process.rs`: one running program; `pool.rs`: the programs of an
//!   extension, and a call's round trip, bounded by `limits.rs`.
//! * `errors.rs`: the conditions of class 38 a call fails with.

pub mod bridge;
pub mod errors;
pub mod host;
pub mod hosted;
pub mod limits;
pub mod manifest;
pub mod package;
pub mod payload;
pub mod plan;
pub mod pool;
mod process;
pub mod settings;
pub mod values;
pub mod wire;

pub use bridge::{Bridged, Installed};
pub use host::Host;
pub use limits::{Directories, Limits};
pub use manifest::{Declared, Manifest, ObjectKind};
pub use pool::{Pool, PoolInfo};

/// \[DESIGN\] The name of the manifest file in a plug-in's directory.
pub const MANIFEST_FILE: &str = "turborel-plugin.manifest";

/// \[DESIGN\] The version of the plug-in protocol and of the manifest's
/// format (VERSIONING.md): both start at 1.
pub const PROTOCOL: u32 = 1;
