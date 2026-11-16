//! Abstract syntax of SQL-server modules [ISO/IEC 9075-4:2006 draft §9.21]
//! (Feature P001).

use super::{RoutineDefinition, TableDefinition, TableName};

/// [ISO/IEC 9075-4:2006 draft §9.21] `<SQL-server module definition>` ::=
/// CREATE MODULE `<SQL-server module name>` \[NAMES ARE `<character set
/// specification>`\] \[SCHEMA `<default schema name>`\] \[PATH `<schema
/// name list>`\] \[`<temporary table declaration>`...\] `<SQL-server module
/// contents>`... END MODULE
#[derive(Debug, Clone)]
pub struct ModuleDefinition {
    /// The `<SQL-server module name>`.
    pub name: TableName,
    /// The `<unqualified schema name>` of SCHEMA, if written \[SR7\].
    pub schema: Option<String>,
    /// The schemas of PATH, if written, each a `<schema name>`'s catalog,
    /// if written, and unqualified name \[SR4, SR5\].
    pub path: Option<Vec<(Option<String>, String)>>,
    /// The `<SQL-server module path specification>` as written, which
    /// MODULES shows.
    pub path_text: Option<String>,
    /// The `<temporary table declaration>`s, in order [§12.8].
    pub temporaries: Vec<TableDefinition>,
    /// The `<module routine>`s, in order [§9.24].
    pub routines: Vec<RoutineDefinition>,
    /// The `<SQL-server module definition>` as written \[GR3h\].
    pub text: String,
}
