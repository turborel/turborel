//! The turborel server (RULES.md D42): a database's sessions for clients
//! over a network, many at once. `turboreld` runs one; a program that
//! embeds the database may serve it too, beside its own sessions.
//!
//! Its extension points (RULES.md R-SAFE-3), beside the database's:
//! `auth::Authenticator`, how clients prove who they are;
//! `transport::Listener`, where connections come from; and
//! `server::Protocol`, how they are spoken to.

pub mod auth;
mod cancels;
pub mod certificate;
mod extensions;
mod holding;
pub mod keepalive;
mod limit;
pub mod listening;
pub mod log;
pub mod logins;
pub mod native;
/// \[EXTENSION\] The cryptography of extension packages (RULES.md D173),
/// which the program serving a database gives it
/// (`Database::set_package_crypto`).
pub use turborel_protocol::packages;
pub mod password;
pub mod replication;
pub mod runtime;
pub mod scram;
pub mod server;
pub mod settings;
pub mod shown;
pub mod tls;
mod told;
pub mod transport;
