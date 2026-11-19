//! \[DESIGN\] A connection URI (RULES.md D202), read into `Options`:
//!
//! ```text
//! turborel://[<user>[:<password>]@][<host>[:<port>][,<host>[:<port>]]...][/][?<key>=<value>[&<key>=<value>]...]
//! ```
//!
//! Its user, password, hosts and values are percent-encoded (RFC 3986
//! §2.1), so a host `%2Frun%2Fturborel.sock` names a Unix socket, and an
//! IPv6 address is written in brackets (RFC 3986 §3.2.2). Each `<key>` is
//! a setting's name (`options::KEYS`). A server serves one database, so
//! the URI names none, and its path is empty, or `/` alone.

use crate::connection::Result;

use super::invalid;
use super::options::{HOST, Options, PASSWORD, USER};

/// The scheme a connection URI begins with.
pub const SCHEME: &str = "turborel://";

/// The value of a hexadecimal digit.
fn hex(digit: u8) -> Option<u8> {
    char::from(digit).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

/// `text` percent-decoded (RFC 3986 §2.1), as UTF-8.
///
/// # Errors
///
/// SQLSTATE 08001 if a `%` is not followed by two hexadecimal digits, or
/// the octets are not UTF-8.
pub(super) fn decode(text: &str) -> Result<String> {
    let mut octets = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        if b != b'%' {
            octets.push(b);
            continue;
        }
        let high = bytes.next().and_then(hex);
        let low = bytes.next().and_then(hex);
        let (Some(high), Some(low)) = (high, low) else {
            return Err(invalid(&format!("[DESIGN] {text} has a % without two hexadecimal digits")));
        };
        octets.push(high.wrapping_mul(16).wrapping_add(low));
    }
    String::from_utf8(octets).map_err(|_| invalid(&format!("[DESIGN] {text} is not UTF-8 once decoded")))
}

impl Options {
    /// The settings the connection URI `uri` gives.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if it is not one: another scheme, a path, a key that
    /// names no setting, a value without `=`, or bad percent-encoding.
    pub fn from_uri(uri: &str) -> Result<Options> {
        let rest = uri.strip_prefix(SCHEME).ok_or_else(|| invalid(&format!("[DESIGN] a connection URI begins with {SCHEME}")))?;
        let (before, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = before.split_once('/').unwrap_or((before, ""));
        if !path.is_empty() {
            return Err(invalid(&format!("[DESIGN] a connection URI names no database, so has no path, not /{path}")));
        }
        let mut options = Options::new();
        let (userinfo, hosts) = authority.rsplit_once('@').unwrap_or(("", authority));
        if !userinfo.is_empty() {
            let (user, password) = userinfo.split_once(':').map_or((userinfo, None), |(u, p)| (u, Some(p)));
            options.set(USER, &decode(user)?)?;
            if let Some(password) = password {
                options.set(PASSWORD, &decode(password)?)?;
            }
        }
        if !hosts.is_empty() {
            options.set(HOST, &decode(hosts)?)?;
        }
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').ok_or_else(|| invalid(&format!("[DESIGN] {pair} in a connection URI has no =")))?;
            options.set(&decode(key)?, &decode(value)?)?;
        }
        Ok(options)
    }
}
