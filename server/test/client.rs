//! The client library's connections beyond one statement at a time
//! (RULES.md D200 to D206): pipelines, settings from outside the program,
//! several servers, TLS by mode, pools and the SCRAM key cache, each
//! against a server.

mod failover;
mod pipelines;
mod pools;
mod tls_modes;
