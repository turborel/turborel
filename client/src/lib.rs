//! A client of the turborel server (RULES.md D42): a blocking connection
//! over the native wire protocol (`turborel_protocol`), on the standard
//! library alone, for programs that connect to a server rather than embed
//! the database; and, with the feature `tls`, over TLS, and with the
//! feature `scram`, proving a password by SCRAM-SHA-256.

pub mod agent;
mod cancel;
pub mod connection;
pub mod import;
pub mod statements;
pub mod target;
pub mod timeouts;

pub use cancel::Canceller;
/// What a connection's results and failures are made of.
pub use turborel_protocol::{Column, Completed, Failure};
#[cfg(feature = "scram")]
pub mod scram;
#[cfg(feature = "tls")]
pub mod tls;
