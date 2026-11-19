//! \[EXTENSION\] The host plug-ins' declarations (RULES.md D229): the
//! kinds of object a host plug-in serves, beneath the database and before
//! its catalog is open, each declared `HOST <kind> <name> [ ( <option> =
//! <value> [ , ... ] ) ]`.

use crate::error::Result;

use super::super::errors::invalid_manifest;

/// The kind of an object a host plug-in serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostKind {
    /// A page encoding (`Encoding`, D246).
    Encoding,
    /// A checksum (`HashFunction`, D246).
    HashFunction,
    /// The clock (`Clock`, D247).
    Clock,
    /// A kind of log, its logs the database's (`LogStorage`, D248).
    LogStorage,
    /// A kind of catalog store (`CatalogStorage`, D248).
    CatalogStorage,
    /// Where work areas spill (`Spill`, D250).
    Spill,
    /// The files a database is opened from (`FileSystem`, D251).
    FileSystem,
    /// What ends and cancels a served connection (`ServedConnections`, D254).
    ServedConnections,
    /// Where a server takes connections, relayed frame by frame (D254).
    Listener,
    /// The cryptography packages are checked by (`PackageCrypto`, D252).
    PackageCrypto,
    /// The keys encryption takes by name (D252).
    Keyring,
    /// Work run unasked for a database (`BackgroundJob`, D253).
    BackgroundJob,
    /// How replicas reach each other (D229).
    ReplicaTransport,
    /// The consensus journal's storage (D229).
    Journal,
    /// How a replica's state is shipped (D229).
    StateTransfer,
    /// The replication protocol itself (D229).
    Consensus,
    /// Where the log is archived and backups kept, beyond a target (D229).
    Archive,
    /// The buffer pool's eviction policy, pushed to the pool (D283).
    BufferEviction,
}

impl HostKind {
    /// Every kind, in the order of their codes.
    pub const ALL: [HostKind; 18] = [
        HostKind::Encoding,
        HostKind::HashFunction,
        HostKind::Clock,
        HostKind::LogStorage,
        HostKind::CatalogStorage,
        HostKind::Spill,
        HostKind::FileSystem,
        HostKind::ServedConnections,
        HostKind::Listener,
        HostKind::PackageCrypto,
        HostKind::Keyring,
        HostKind::BackgroundJob,
        HostKind::ReplicaTransport,
        HostKind::Journal,
        HostKind::StateTransfer,
        HostKind::Consensus,
        HostKind::Archive,
        HostKind::BufferEviction,
    ];

    /// The kind's code in protocol 1, after the extensions' 1 to 19.
    pub fn code(self) -> u8 {
        match self {
            HostKind::Encoding => 20,
            HostKind::HashFunction => 21,
            HostKind::Clock => 22,
            HostKind::LogStorage => 23,
            HostKind::CatalogStorage => 24,
            HostKind::Spill => 25,
            HostKind::FileSystem => 26,
            HostKind::ServedConnections => 27,
            HostKind::Listener => 28,
            HostKind::PackageCrypto => 29,
            HostKind::Keyring => 30,
            HostKind::BackgroundJob => 31,
            HostKind::ReplicaTransport => 32,
            HostKind::Journal => 33,
            HostKind::StateTransfer => 34,
            HostKind::Consensus => 35,
            HostKind::Archive => 36,
            HostKind::BufferEviction => 38,
        }
    }

    /// The kind of `code`, if it is one.
    pub fn of_code(code: u8) -> Option<HostKind> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }

    /// The words a declaration of it begins with, after HOST.
    pub fn words(self) -> &'static [&'static str] {
        match self {
            HostKind::Encoding => &["ENCODING"],
            HostKind::HashFunction => &["HASH", "FUNCTION"],
            HostKind::Clock => &["CLOCK"],
            HostKind::LogStorage => &["LOG", "STORAGE"],
            HostKind::CatalogStorage => &["CATALOG", "STORAGE"],
            HostKind::Spill => &["SPILL"],
            HostKind::FileSystem => &["FILE", "SYSTEM"],
            HostKind::ServedConnections => &["SERVED", "CONNECTIONS"],
            HostKind::Listener => &["LISTENER"],
            HostKind::PackageCrypto => &["PACKAGE", "CRYPTO"],
            HostKind::Keyring => &["KEYRING"],
            HostKind::BackgroundJob => &["BACKGROUND", "JOB"],
            HostKind::ReplicaTransport => &["REPLICA", "TRANSPORT"],
            HostKind::Journal => &["JOURNAL"],
            HostKind::StateTransfer => &["STATE", "TRANSFER"],
            HostKind::Consensus => &["CONSENSUS"],
            HostKind::Archive => &["ARCHIVE"],
            HostKind::BufferEviction => &["BUFFER", "EVICTION"],
        }
    }

    /// Its words as one name, as views and messages show it.
    pub fn word(self) -> String {
        self.words().join(" ")
    }
}

/// The value of a host declaration's option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostOption {
    /// An unsigned integer.
    Number(u64),
    /// A character string.
    Text(String),
}

/// An object a host plug-in's manifest declares.
#[derive(Debug, Clone)]
pub struct HostDeclared {
    /// Its kind.
    pub kind: HostKind,
    /// Its name.
    pub name: String,
    /// Its options, by name, in the order written.
    pub options: Vec<(String, HostOption)>,
    /// The declaration as written.
    pub text: String,
}

impl HostDeclared {
    /// The number `option` gives, or `default` if it is not written.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if it is written as a character string.
    pub fn number(&self, option: &str, default: u64) -> Result<u64> {
        match self.option(option) {
            None => Ok(default),
            Some(HostOption::Number(n)) => Ok(*n),
            Some(HostOption::Text(_)) => Err(invalid_manifest(&format!("{} {}: {option} is a number", self.kind.word(), self.name))),
        }
    }

    /// The character string `option` gives, if it is written.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if it is written as a number.
    pub fn text(&self, option: &str) -> Result<Option<&str>> {
        match self.option(option) {
            None => Ok(None),
            Some(HostOption::Text(t)) => Ok(Some(t)),
            Some(HostOption::Number(_)) => {
                Err(invalid_manifest(&format!("{} {}: {option} is a character string", self.kind.word(), self.name)))
            }
        }
    }

    /// The value of `option`, if it is written.
    fn option(&self, option: &str) -> Option<&HostOption> {
        self.options.iter().find(|(n, _)| n.eq_ignore_ascii_case(option)).map(|(_, v)| v)
    }
}
