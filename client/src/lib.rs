//! A client of the turborel server (RULES.md D42): a blocking connection
//! over the native wire protocol (`turborel_protocol`), on the standard
//! library alone, for programs that connect to a server rather than embed
//! the database; and, with the feature `tls`, over TLS.

mod cancel;
pub mod connection;
pub mod statements;
pub mod target;

pub use cancel::Canceller;
/// What a connection's results and failures are made of.
pub use turborel_protocol::{Column, Completed, Failure};
#[cfg(feature = "tls")]
pub mod tls;
