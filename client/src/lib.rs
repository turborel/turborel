//! A client of the turborel server (RULES.md D42): a blocking connection
//! over the native wire protocol (`turborel_protocol`), on the standard
//! library alone, for programs that connect to a server rather than embed
//! the database; and, with the feature `tls`, over TLS, and with the
//! feature `scram`, proving a password by SCRAM-SHA-256. Settings may come
//! from outside the program (`settings`), name several servers
//! (`failover`), and requests be pipelined (`pipeline`) and connections
//! pooled (`pool`).

pub mod agent;
mod cancel;
pub mod connection;
pub mod copy;
pub mod export;
mod failover;
mod frames;
pub mod import;
pub mod lobs;
pub mod mechanism;
#[cfg(any(windows, test))]
mod pipe;
pub mod pipeline;
pub mod pool;
pub mod role;
pub mod session;
pub mod settings;
pub mod statements;
pub mod target;
pub mod timeouts;
pub mod told;

pub use cancel::Canceller;
/// The capabilities a client asks for and a server grants (RULES.md D185).
pub use turborel_protocol::capability;
/// The splitter of text into statements the server uses for a Query of
/// several statements, for a program to split text alike.
pub use turborel_protocol::split::Splitter;
/// What a connection's results, failures, notices and server times are
/// made of.
pub use turborel_protocol::{Column, Completed, Diagnostics, Failure, Notice, Timing};
/// The cryptography of extension packages (`turborel_protocol::packages`).
#[cfg(feature = "tls")]
pub use turborel_protocol::packages;
#[cfg(feature = "scram")]
pub mod scram;
#[cfg(feature = "tls")]
pub mod tls;
