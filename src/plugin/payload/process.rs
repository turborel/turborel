//! \[DESIGN\] The payloads of the kinds that serve the database as a
//! whole (RULES.md D222): what makes users' verifiers, and a sink of its
//! events.
//!
//! ```text
//! CREDENTIALS 1 verifier  user str | password str               -> bytes
//! DIAGNOSTICS 1 events    count u32 | (level str | kind str | text str)... | dropped u64  -> (nothing)
//! ```
//!
//! `dropped` counts the events the sink let go since its last call, as
//! its queue was full: an event never waits on a program.

use crate::bytes::{Put, Reader};
use crate::error::Result;

use super::count;

/// An event, as a sink's program is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    /// Its level's word: ERROR, WARNING or INFO.
    pub level: String,
    /// Its kind, the name of its variant.
    pub kind: String,
    /// What it says, all of it.
    pub text: String,
}

/// VERIFIER's payload.
pub fn put_verifier(out: &mut Vec<u8>, user: &str, password: &str) {
    out.put_str(user);
    out.put_str(password);
}

/// What `put_verifier` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn verifier(r: &mut Reader<'_>) -> Result<(String, String)> {
    Ok((r.str()?, r.str()?))
}

/// EVENTS's payload.
pub fn put_events(out: &mut Vec<u8>, events: &[Told], dropped: u64) {
    out.put_u32(count(events.len()));
    for e in events {
        out.put_str(&e.level);
        out.put_str(&e.kind);
        out.put_str(&e.text);
    }
    out.put_u64(dropped);
}

/// What `put_events` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn events(r: &mut Reader<'_>) -> Result<(Vec<Told>, u64)> {
    let told = (0..r.u32()?).map(|_| Ok(Told { level: r.str()?, kind: r.str()?, text: r.str()? })).collect::<Result<_>>()?;
    Ok((told, r.u64()?))
}
