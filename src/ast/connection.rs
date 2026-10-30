//! Abstract syntax of the connection statements [ISO/IEC 9075:1992 §15].
//! Split from ast.rs (RULES.md R-SIZE-2).

use super::ValueSpecification;

/// [ISO/IEC 9075:1992 §15] A statement of an SQL-agent's connections.
#[derive(Debug, Clone)]
pub enum ConnectionStatement {
    /// [§15.1] CONNECT TO `<SQL-server name>` \[AS `<connection name>`\]
    /// \[USER `<user name>`\], each a `<simple value specification>`.
    Connect {
        /// The `<SQL-server name>`.
        server: ValueSpecification,
        /// The `<connection name>`, if written.
        name: Option<ValueSpecification>,
        /// The `<user name>`, if written.
        user: Option<ValueSpecification>,
    },
    /// [§15.1] CONNECT TO DEFAULT.
    ConnectDefault,
    /// [§15.2] SET CONNECTION `<connection name>`, or DEFAULT for none.
    Set(Option<ValueSpecification>),
    /// [§15.3] DISCONNECT `<disconnect object>`.
    Disconnect(Disconnected),
}

/// [ISO/IEC 9075:1992 §15.3] A `<disconnect object>`.
#[derive(Debug, Clone)]
pub enum Disconnected {
    /// A `<connection name>`.
    Named(ValueSpecification),
    /// DEFAULT.
    Default,
    /// ALL.
    All,
    /// CURRENT.
    Current,
}

/// \[EXTENSION\] A statement of a connection a server serves (RULES.md
/// D42), by the database's owner, its connection named by its ID in
/// INFORMATION_SCHEMA.CONNECTIONS. ISO/IEC 9075:1992 has none.
#[derive(Debug)]
pub enum ServedStatement {
    /// `<end connection statement>` ::= END CONNECTION `<connection
    /// identifier>`
    End {
        /// The `<connection identifier>`, and where it begins.
        id: (u64, usize),
    },
    /// `<cancel connection statement>` ::= CANCEL CONNECTION `<connection
    /// identifier>`
    Cancel {
        /// The `<connection identifier>`, and where it begins.
        id: (u64, usize),
    },
}
