//! \[DESIGN\] The values of connection settings that are one of a few
//! words (RULES.md D202): how TLS is used, whether SCRAM binds its
//! channel, what a server must be to be taken, and whether servers are
//! tried in order. Each is written in lower case, as a setting, the
//! environment and a URI give it.

use crate::connection::Result;

use super::invalid;

/// How a connection over TCP uses TLS (RULES.md D204). A local
/// connection, over a Unix socket or a named pipe, is private, and uses
/// none whatever this says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// Never: plain TCP.
    Disable,
    /// TLS if the server takes it, else plain TCP; its certificate not
    /// checked.
    #[default]
    Prefer,
    /// TLS or no connection; the server's certificate not checked.
    Require,
    /// TLS, the server's certificate signed by an authority trusted, its
    /// name not checked.
    VerifyCa,
    /// TLS, the server's certificate signed by an authority trusted and
    /// of the server's name.
    VerifyFull,
}

/// Whether SCRAM binds its exchange to the TLS connection carrying it
/// (RULES.md D204).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelBinding {
    /// Never: `scram-sha-256` even over TLS.
    Disable,
    /// Over TLS: `scram-sha-256-plus` where the connection has a binding.
    #[default]
    Prefer,
    /// Always: a connection without a binding, or a mechanism that binds
    /// none, is refused before it proves anything.
    Require,
}

/// What a server must be for a connection to it to be kept (RULES.md
/// D201).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetRole {
    /// Any server.
    #[default]
    Any,
    /// One that takes changes: a server alone, or its group's primary.
    ReadWrite,
    /// One that takes none: a replica following its group's log.
    ReadOnly,
}

/// Whether servers are tried in the order given (RULES.md D201).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadBalance {
    /// In order.
    #[default]
    Disable,
    /// In an order chosen at random for each connection, so connections
    /// spread over them.
    Random,
}

/// The words of `TlsMode`, in its order.
const TLS_MODES: [(&str, TlsMode); 5] = [
    ("disable", TlsMode::Disable),
    ("prefer", TlsMode::Prefer),
    ("require", TlsMode::Require),
    ("verify-ca", TlsMode::VerifyCa),
    ("verify-full", TlsMode::VerifyFull),
];
/// The words of `ChannelBinding`.
const CHANNEL_BINDINGS: [(&str, ChannelBinding); 3] =
    [("disable", ChannelBinding::Disable), ("prefer", ChannelBinding::Prefer), ("require", ChannelBinding::Require)];
/// The words of `TargetRole`.
const TARGET_ROLES: [(&str, TargetRole); 3] =
    [("any", TargetRole::Any), ("read-write", TargetRole::ReadWrite), ("read-only", TargetRole::ReadOnly)];
/// The words of `LoadBalance`.
const LOAD_BALANCES: [(&str, LoadBalance); 2] = [("disable", LoadBalance::Disable), ("random", LoadBalance::Random)];
/// The words of a yes or no.
const BOOLEANS: [(&str, bool); 2] = [("true", true), ("false", false)];

/// The value of `key` the word `text` is, among `words`.
fn word<T: Copy>(key: &str, text: &str, words: &[(&str, T)]) -> Result<T> {
    words.iter().find(|(w, _)| *w == text).map(|(_, v)| *v).ok_or_else(|| {
        let known: Vec<&str> = words.iter().map(|(w, _)| *w).collect();
        invalid(&format!("[DESIGN] {key} is {text}, not one of {}", known.join(", ")))
    })
}

impl TlsMode {
    /// The mode `text` names, the value of `key`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if it names none.
    pub fn parse(key: &str, text: &str) -> Result<TlsMode> {
        word(key, text, &TLS_MODES)
    }

    /// Whether the server's certificate is checked against the
    /// authorities trusted.
    pub fn verifies(self) -> bool {
        match self {
            TlsMode::VerifyCa | TlsMode::VerifyFull => true,
            TlsMode::Disable | TlsMode::Prefer | TlsMode::Require => false,
        }
    }
}

impl ChannelBinding {
    /// The binding `text` names, the value of `key`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if it names none.
    pub fn parse(key: &str, text: &str) -> Result<ChannelBinding> {
        word(key, text, &CHANNEL_BINDINGS)
    }
}

impl TargetRole {
    /// The role `text` names, the value of `key`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if it names none.
    pub fn parse(key: &str, text: &str) -> Result<TargetRole> {
        word(key, text, &TARGET_ROLES)
    }
}

impl LoadBalance {
    /// The choice `text` names, the value of `key`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if it names none.
    pub fn parse(key: &str, text: &str) -> Result<LoadBalance> {
        word(key, text, &LOAD_BALANCES)
    }
}

/// The yes or no `text` is, `true` or `false`, the value of `key`.
///
/// # Errors
///
/// SQLSTATE 08001 if it is neither.
pub(super) fn boolean(key: &str, text: &str) -> Result<bool> {
    word(key, text, &BOOLEANS)
}
