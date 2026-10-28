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
