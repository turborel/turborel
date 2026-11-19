//! \[EXTENSION\] The plug-in SDK (RULES.md D167, D220): what a program
//! written in Rust needs to serve turborel's external plug-ins over
//! protocol 1, each object of its manifest an implementation of the trait
//! of its kind, the protocol's frames, handshake, values, batches,
//! errors, cancellation and limits the SDK's.
//!
//! A program builds a `Plugin` of its objects, by the names its manifest
//! declares, and hands it to `serve`, which reads its standard input and
//! writes its standard output until the database shuts it down:
//!
//! ```no_run
//! use turborel::value::Value;
//! use turborel_plugin::{Answer, Cx, Plugin, Scalar, serve};
//!
//! struct Twice;
//!
//! impl Scalar for Twice {
//!     fn call(&mut self, _cx: &mut Cx<'_>, args: &[Value]) -> Answer<Value> {
//!         match args.first() {
//!             Some(Value::Exact(d)) => Ok(Value::Exact(d.checked_mul(turborel::value::Decimal::new(2, 0))?)),
//!             Some(_) | None => Ok(Value::Null),
//!         }
//!     }
//! }
//!
//! fn main() {
//!     let plugin = Plugin::new("1.0.0").scalar("TWICE", Twice);
//!     if serve(plugin).is_err() {
//!         std::process::exit(1);
//!     }
//! }
//! ```
//!
//! A batch the database sends, as the rows of a scalar function's CALL,
//! is answered a call of the trait for each, so an author writes one
//! row's answer. Each call is given a `Cx`: whether the database asked to
//! cancel the call, the SQL a procedure or table function runs in the
//! call's transaction, and the limits HELLO gave. An error is a
//! `PluginError`, its SQLSTATE kept by the database if of a class a
//! routine may raise (RULES.md D172). `Harness` runs a `Plugin` in the
//! test's own process, so its objects are tested with no database.
//!
//! The SDK changes nothing of the protocol: its frames are those of
//! `turborel::plugin::wire`, its payloads those of
//! `turborel::plugin::payload`, so a program in another language serves
//! the same (`doc/client/04-plugins.md` shows one written by hand).

mod dispatch;
mod harness;
pub mod host;
mod input;
mod kinds;
mod objects;
mod registry;
mod rule;
mod serve;
mod storage;

use turborel::error::SqlError;

pub use harness::Harness;
pub use host::HostObject;
pub use host::replication::{Consensus, Journal, ReplicaTransport, StateTransfer};
pub use kinds::{
    Aggregate, Authenticator, Collation, Credentials, Diagnostics, Embedder, Exchange, Metric, PartitionStrategy, Procedure, Quantizer,
    Ranking, Scalar, Step, SystemTable, TableFunction, Target, TargetStore, Type, Upload, Who, XQueryFunction,
};
pub use registry::Plugin;
pub use rule::Rule;
pub use serve::{Cx, Limits, serve, serve_on};
pub use storage::{Engine, Entries, IndexMethod, Search};
pub use turborel::plugin::payload::engine::Opened;
pub use turborel::plugin::payload::index::{Near, Probed, Write};
pub use turborel::plugin::payload::process::Told;
pub use turborel::plugin::payload::quantizer::{Trained, Training};
pub use turborel::plugin::payload::replication::consensus::{Event as ReplicaEvent, Input as ReplicaInput, Output as ReplicaOutput};
pub use turborel::plugin::payload::replication::{Held, Manifest, Member, Served};
pub use turborel::plugin::payload::statistics::{ColumnAnswer, IndexAnswer, TableAnswer};
pub use turborel::plugin::plan;
pub use turborel::plugin::payload::types::Made;
pub use turborel::plugin::values::{coordinates, dense_vector};

/// \[EXTENSION\] Why a call failed: a SQLSTATE and a message, sent as its
/// ERROR. The database keeps the state if it is of a class a routine may
/// raise (22, 23, 38, 39, 42, 2F), else raises 38000 (RULES.md D172).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    /// The SQLSTATE, five characters.
    pub state: String,
    /// Why, for people.
    pub message: String,
}

impl PluginError {
    /// An error of `state` and `message`.
    pub fn new(state: &str, message: &str) -> PluginError {
        PluginError { state: state.to_owned(), message: message.to_owned() }
    }

    /// A data exception, SQLSTATE 22000, of `message`.
    pub fn data(message: &str) -> PluginError {
        PluginError::new("22000", message)
    }

    /// An external routine exception, SQLSTATE 38000, of `message`: what
    /// the program cannot do.
    pub fn external(message: &str) -> PluginError {
        PluginError::new("38000", message)
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.state, self.message)
    }
}

impl From<SqlError> for PluginError {
    fn from(e: SqlError) -> PluginError {
        PluginError { state: e.sqlstate().to_string(), message: e.msg }
    }
}

/// What a call of an object answers.
pub type Answer<T> = Result<T, PluginError>;

#[cfg(test)]
mod tests;
