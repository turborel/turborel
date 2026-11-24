//! \[DESIGN\] Capabilities (RULES.md D185): what a client asks for in
//! Hello, by name, and the server grants in Ready, a subset of those
//! asked. Each wire behaviour added to version 1 after its first form has
//! a capability of its own, so a peer that does not ask never sees it,
//! and the protocol's version stays 1. A name a server does not know is
//! not granted, and a client never sends what was not granted.
//!
//! Connection attributes in Hello are the exception: they are sent
//! before anything is granted, so every server of version 1 reads them,
//! and the client asks for `CONNECTION_ATTRIBUTES` only to say so.

/// The server sends a Notice for each completion condition of class 01 a
/// statement raises, and for its own notices, before the statement's
/// completion.
pub const NOTICES: &str = "notices";
/// The server sends a Status for each of the session's settings as the
/// session begins, and again for each one a statement changes.
pub const STATUS: &str = "status";
/// An Error carries the failure's diagnostics: severity, detail, hint,
/// context, and the names of what it is about.
pub const DIAGNOSTICS: &str = "diagnostics";
/// Describe, of a prepared statement or of an open cursor.
pub const DESCRIBE: &str = "describe";
/// Several cursors open at once of one prepared statement, each numbered
/// by the client in Execute, Fetch and Close.
pub const CURSORS: &str = "cursors";
/// Ping, answered by Pong without a worker.
pub const PING: &str = "ping";
/// Reset: the session back to its state at login, its authentication
/// kept.
pub const RESET: &str = "reset";
/// ChangeUser: the connection authenticated again, as another user.
pub const CHANGE_USER: &str = "change-user";
/// Attributes of a Query or an Execute, read in SQL by QUERY_ATTRIBUTE.
pub const QUERY_ATTRIBUTES: &str = "query-attributes";
/// Attributes of the connection, sent in Hello.
pub const CONNECTION_ATTRIBUTES: &str = "connection-attributes";
/// A Query of several statements, run one by one.
pub const MULTI_STATEMENT: &str = "multi-statement";
/// Frames past a threshold compressed, both ways.
pub const COMPRESSION: &str = "compression";
/// A Complete that committed carries the log position of its commit.
pub const COMMIT_POSITION: &str = "commit-position";
/// CopyIn, CopyData and CopyEnd: rows into a table without a round trip
/// per batch.
pub const COPY: &str = "copy";

/// A Complete carries the rows its statement inserted, updated, deleted
/// or merged, as GET DIAGNOSTICS ROW_COUNT gives them, or the rows a
/// query returned.
pub const ROW_COUNTS: &str = "row-counts";
/// A Complete of an INSERT carries the identity values it generated.
pub const GENERATED: &str = "generated-values";
/// An Execute that ends the transaction with it, whose cursor is left
/// open, ends it as a Fetch reads its last row or a Close closes it.
pub const FETCH_ENDS: &str = "fetch-ends";
/// A Complete carries each condition its statement raised, in place of a
/// Notice for each.
pub const CONDITIONS: &str = "conditions";
/// \[DESIGN\] A Description's and a Prepared's columns carry the type each
/// was declared with, as INTEGER, which its tag alone does not tell
/// (RULES.md D396).
pub const DECLARED_TYPES: &str = "declared-types";
/// \[DESIGN\] A Prepared of a CALL carries its procedure's specific name
/// and each of its `?`s with the mode and ordinal position of its
/// parameter [ISO/IEC 9075-2:2023 §20.10 GR7e] (RULES.md D978).
pub const PARAMETER_MODES: &str = "parameter-modes";

/// \[EXTENSION\] Debug: one connection attaches to another's session and
/// drives the routines it runs, breakpoints, steps and the call stack, and
/// is sent an Event, unasked, as that session pauses (RULES.md D1030).
pub const DEBUGGER: &str = "debugger";

/// Every capability of version 1, in the order of their bits.
pub const ALL: [&str; 21] = [
    NOTICES,
    STATUS,
    DIAGNOSTICS,
    DESCRIBE,
    CURSORS,
    PING,
    RESET,
    CHANGE_USER,
    QUERY_ATTRIBUTES,
    CONNECTION_ATTRIBUTES,
    MULTI_STATEMENT,
    COMPRESSION,
    COMMIT_POSITION,
    COPY,
    ROW_COUNTS,
    GENERATED,
    FETCH_ENDS,
    CONDITIONS,
    DECLARED_TYPES,
    PARAMETER_MODES,
    DEBUGGER,
];

/// \[DESIGN\] A set of capabilities, one bit for each of `ALL`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities(u32);

/// The bit of the capability at `at` in `ALL`.
fn bit(at: usize) -> u32 {
    u32::try_from(at).ok().and_then(|at| 1_u32.checked_shl(at)).unwrap_or(0)
}

impl Capabilities {
    /// None.
    pub const NONE: Capabilities = Capabilities(0);

    /// Every capability of version 1.
    pub fn all() -> Capabilities {
        Capabilities::of(&ALL)
    }

    /// The capabilities among `names` this side knows: others are left
    /// out, so a server grants none it does not know.
    pub fn of<S: AsRef<str>>(names: &[S]) -> Capabilities {
        let mut set = Capabilities::NONE;
        for name in names {
            if let Some(at) = ALL.iter().position(|known| *known == name.as_ref()) {
                set.0 |= bit(at);
            }
        }
        set
    }

    /// Whether it has the capability `name`.
    pub fn has(self, name: &str) -> bool {
        ALL.iter().position(|known| *known == name).is_some_and(|at| self.0 & bit(at) != 0)
    }

    /// Those of it `other` has too.
    #[must_use]
    pub fn and(self, other: Capabilities) -> Capabilities {
        Capabilities(self.0 & other.0)
    }

    /// It, `name` left out.
    #[must_use]
    pub fn without(self, name: &str) -> Capabilities {
        let at = ALL.iter().position(|known| *known == name);
        Capabilities(at.map_or(self.0, |at| self.0 & !bit(at)))
    }

    /// Its names, in the order of `ALL`, as Hello and Ready list them.
    pub fn names(self) -> Vec<String> {
        ALL.iter().enumerate().filter(|(at, _)| self.0 & bit(*at) != 0).map(|(_, name)| (*name).to_owned()).collect()
    }
}
