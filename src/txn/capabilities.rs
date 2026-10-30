//! \[DESIGN\] What a storage engine does for transactions, and what the
//! transaction policy accepts when an engine falls short of [X3.135-1986
//! §4.16]: how changes are undone, who isolates transactions, and how data
//! survives a crash (RULES.md D25, D29). Split from txn.rs (RULES.md
//! R-SIZE-2).

/// How an engine's changes are cancelled on ROLLBACK or statement failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undo {
    /// The coordinator's undo log calls `delete`, `restore` and `update`.
    Log,
    /// The engine keeps its own undo; see the transaction methods of `TableStorage`.
    Native,
    /// Changes cannot be cancelled (an append-only file, a remote API).
    Unsupported,
}

/// Who keeps concurrent transactions from interfering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concurrency {
    /// The coordinator's table locks (serializable).
    Coordinator,
    /// The engine itself, for example by multi-versioning.
    Engine {
        /// Whether the engine's isolation is serializable [X3.135-1986 §4.16].
        serializable: bool,
    },
}

/// \[DESIGN\] How an engine's data survives a crash (RULES.md D25): its
/// durability, as a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    /// Recovered by the database, replaying the log from the position the
    /// engine is durable through (`TableStorage::durable_through`): as
    /// durable as the log. The built-in row store is durable through its
    /// last checkpoint (RULES.md D25): the log is replayed from there.
    Logged,
    /// The engine makes its own writes durable when a transaction ends,
    /// committed or rolled back (a rollback's undo is written through the
    /// engine like any change); recovery need only undo the changes of a
    /// transaction a crash interrupted, from the log's before-images.
    Forced,
    /// The engine keeps its own durability and atomicity; its changes are
    /// not logged by the database.
    External,
    /// Never recovered: scratch data. Its changes are not logged.
    None,
}

/// What an engine does for transactions itself, and what it leaves to the
/// coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxCapabilities {
    /// How the engine's changes are cancelled.
    pub undo: Undo,
    /// Who isolates concurrent transactions on the engine's tables.
    pub concurrency: Concurrency,
    /// How the engine's data survives a crash.
    pub persistence: Persistence,
}

impl Default for TxCapabilities {
    fn default() -> Self {
        TxCapabilities { undo: Undo::Log, concurrency: Concurrency::Coordinator, persistence: Persistence::Logged }
    }
}

/// What to do when an engine cannot give the guarantees of [X3.135-1986 §4.16].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Refuse: no writes to an engine that cannot cancel changes, no use of an
    /// engine whose isolation is weaker than serializable.
    Strict,
    /// Allow it. "An implementation remains conforming even if it provides
    /// user options to process conforming SQL language in a nonconforming
    /// manner" [X3.135-1986 §3.4].
    Permissive,
}
