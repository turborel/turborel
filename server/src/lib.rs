//! The turborel server (RULES.md D42): a database's sessions for clients
//! over a network, many at once. `turboreld` runs one; a program that
//! embeds the database may serve it too, beside its own sessions.
//!
//! Its extension points (RULES.md R-SAFE-3), beside the database's:
//! `auth::Authenticator`, how clients prove who they are;
//! `transport::Listener`, where connections come from; and
//! `server::Protocol`, how they are spoken to.

pub mod auth;
mod limit;
pub mod listening;
pub mod log;
pub mod native;
pub mod password;
pub mod runtime;
pub mod server;
pub mod settings;
pub mod shown;
pub mod tls;
pub mod transport;
