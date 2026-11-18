//! How the diagnostics area names a statement [ISO/IEC 9075:1992 §18.1
//! GR1d, Table 22]: COMMAND_FUNCTION of each statement, and
//! DYNAMIC_FUNCTION of one prepared.

use super::super::area::commands::{self, Command};
use super::super::dynamic;
use crate::ast::{
    ConnectionStatement, DropObject, IndexStatement, RoleStatement, SavepointStatement, SchemaStatement, ServedStatement, SqlStatement,
    UserStatement, VectorObject, VectorStatement,
};
use crate::lexer::{Tok, Token, kw, lex};

/// [ISO/IEC 9075:1992 §18.1 GR1d, Table 22] How DYNAMIC_FUNCTION names
/// `stmt`, prepared: a positioned DELETE or UPDATE prepared is a
/// `<preparable dynamic delete statement: positioned>` or `<preparable
/// dynamic update statement: positioned>` [§17.6, §17.19, §17.20]; any
/// other as COMMAND_FUNCTION names it.
pub(super) fn prepared_command(stmt: &SqlStatement) -> Command {
    if let SqlStatement::DeletePositioned { .. } = stmt {
        return commands::PREPARABLE_DELETE_CURSOR;
    }
    if let SqlStatement::UpdatePositioned { .. } = stmt {
        return commands::PREPARABLE_UPDATE_CURSOR;
    }
    command_of(stmt)
}

/// Whether `sql` begins as a `<get diagnostics statement>` does.
pub(super) fn diagnostics_statement(sql: &str) -> bool {
    let tokens = lex(sql).unwrap_or_default();
    matches!(tokens.as_slice(), [Token { tok: Tok::KeyWord(kw::GET), .. }, Token { tok: Tok::KeyWord(kw::DIAGNOSTICS), .. }, ..])
}

/// How COMMAND_FUNCTION names schema statement `s` (`command_of`).
fn schema_command(s: &SchemaStatement) -> Command {
    match s {
        SchemaStatement::Assertion(_) => commands::CREATE_ASSERTION,
        SchemaStatement::Collation(_) => commands::CREATE_COLLATION,
        SchemaStatement::Domain(_) => commands::CREATE_DOMAIN,
        SchemaStatement::AlterDomain(_) => commands::ALTER_DOMAIN,
        SchemaStatement::Table(_) => commands::CREATE_TABLE,
        SchemaStatement::View(_) => commands::CREATE_VIEW,
        SchemaStatement::Grant(_) => commands::GRANT,
        SchemaStatement::Revoke(_) => commands::REVOKE,
        SchemaStatement::Role(RoleStatement::Create { .. }) => commands::CREATE_ROLE,
        SchemaStatement::Role(RoleStatement::Grant { .. }) => commands::GRANT_ROLE,
        SchemaStatement::Role(RoleStatement::Revoke { .. }) => commands::REVOKE_ROLE,
        SchemaStatement::Role(RoleStatement::Drop { .. }) => commands::DROP_ROLE,
        SchemaStatement::Sequence(_) => commands::CREATE_SEQUENCE,
        SchemaStatement::AlterSequence(_) => commands::ALTER_SEQUENCE,
        SchemaStatement::Type(_) => commands::CREATE_TYPE,
        SchemaStatement::AlterType(_) => commands::ALTER_TYPE,
        SchemaStatement::Cast(_) => commands::CREATE_CAST,
        SchemaStatement::Ordering(_) => commands::CREATE_ORDERING,
        SchemaStatement::Transform(_) => commands::CREATE_TRANSFORM,
        SchemaStatement::AlterTransform(_) => commands::ALTER_TRANSFORM,
        SchemaStatement::Trigger(_) => commands::CREATE_TRIGGER,
        SchemaStatement::Method(_) | SchemaStatement::Routine(_) => commands::CREATE_ROUTINE,
        SchemaStatement::Module(_) => commands::CREATE_MODULE,
        SchemaStatement::Vector(v) => match **v {
            VectorStatement::Model(_) => commands::CREATE_EMBEDDING_MODEL,
            VectorStatement::AlterModel(_) => commands::ALTER_EMBEDDING_MODEL,
            VectorStatement::Pipeline(_) => commands::CREATE_VECTOR_PIPELINE,
        },
        SchemaStatement::XmlSchema(_) => commands::CREATE_XML_SCHEMA,
        SchemaStatement::XQueryModule(_) => commands::CREATE_XQUERY_MODULE,
    }
}

/// [ISO/IEC 9075-2:2023 §23.1 GR2c, GR2d, Table 39] How COMMAND_FUNCTION
/// and COMMAND_FUNCTION_CODE name the drop of `object`.
fn drop_command(object: &DropObject) -> Command {
    match object {
        DropObject::Table(_) => commands::DROP_TABLE,
        DropObject::View(_) => commands::DROP_VIEW,
        DropObject::Schema { .. } => commands::DROP_SCHEMA,
        DropObject::Domain(_) => commands::DROP_DOMAIN,
        DropObject::Assertion(_) => commands::DROP_ASSERTION,
        DropObject::Collation(_) => commands::DROP_COLLATION,
        DropObject::Sequence(_) => commands::DROP_SEQUENCE,
        DropObject::Type(_) => commands::DROP_TYPE,
        DropObject::Trigger(_) => commands::DROP_TRIGGER,
        DropObject::Routine(_) => commands::DROP_ROUTINE,
        DropObject::Module(_) => commands::DROP_MODULE,
        DropObject::Cast(..) => commands::DROP_CAST,
        DropObject::Ordering(_) => commands::DROP_ORDERING,
        DropObject::Transform(..) => commands::DROP_TRANSFORM,
        DropObject::Vector(VectorObject::Model, _) => commands::DROP_EMBEDDING_MODEL,
        DropObject::Vector(VectorObject::Pipeline, _) => commands::DROP_VECTOR_PIPELINE,
        DropObject::XmlSchema(_) => commands::DROP_XML_SCHEMA,
        DropObject::XQueryModule(_) => commands::DROP_XQUERY_MODULE,
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR1c, Table 22; ISO/IEC 9075-2:2023 §23.1
/// GR2c, GR2d, Table 39] How COMMAND_FUNCTION and COMMAND_FUNCTION_CODE
/// name `stmt`.
pub(in crate::db) fn command_of(stmt: &SqlStatement) -> Command {
    match stmt {
        SqlStatement::Control(c) => commands::control(c),
        SqlStatement::Signal(s) => commands::signal(s.resignal),
        SqlStatement::Close(_) => commands::CLOSE_CURSOR,
        SqlStatement::Schema { statement, .. } => schema_command(statement),
        SqlStatement::Commit => commands::COMMIT_WORK,
        SqlStatement::DeletePositioned { .. } => commands::DELETE_CURSOR,
        SqlStatement::DeleteSearched { .. } => commands::DELETE_WHERE,
        SqlStatement::Truncate { .. } => commands::TRUNCATE_TABLE,
        SqlStatement::AlterTable { .. } => commands::ALTER_TABLE,
        SqlStatement::Drop { object, .. } => drop_command(object),
        SqlStatement::Fetch { .. } => commands::FETCH,
        SqlStatement::GetDiagnostics(_) => commands::GET_DIAGNOSTICS,
        SqlStatement::Dynamic(d) => dynamic::command(d),
        SqlStatement::Execute(e) => dynamic::execute_command(e),
        SqlStatement::DeclareTable(_) => commands::DECLARE_TABLE,
        SqlStatement::Insert(_) => commands::INSERT,
        SqlStatement::Merge(_) => commands::MERGE,
        SqlStatement::Open(_) => commands::OPEN,
        SqlStatement::Rollback | SqlStatement::Savepoint(SavepointStatement::RollbackTo(_)) => commands::ROLLBACK_WORK,
        SqlStatement::Savepoint(SavepointStatement::Establish(_)) => commands::SAVEPOINT,
        SqlStatement::Savepoint(SavepointStatement::Release(_)) => commands::RELEASE_SAVEPOINT,
        SqlStatement::Select { .. } => commands::SELECT_ROW,
        SqlStatement::SetTransaction(_) => commands::SET_TRANSACTION,
        SqlStatement::SetSchema(_) => commands::SET_SCHEMA,
        SqlStatement::SetCollation(_) => commands::SET_COLLATION,
        SqlStatement::SetTransformGroup(_) => commands::SET_TRANSFORM_GROUP,
        SqlStatement::SetCatalog(_) => commands::SET_CATALOG,
        SqlStatement::SetTimeZone(_) => commands::SET_TIME_ZONE,
        SqlStatement::Locator { hold: false, .. } => commands::FREE_LOCATOR,
        SqlStatement::Locator { hold: true, .. } => commands::HOLD_LOCATOR,
        SqlStatement::SetSessionAuthorization(_) => commands::SET_SESSION_AUTHORIZATION,
        SqlStatement::SetRole(_) => commands::SET_ROLE,
        SqlStatement::Connection(c) => connection_command(c),
        SqlStatement::SetConstraints { .. } => commands::SET_CONSTRAINT,
        SqlStatement::UpdatePositioned { .. } => commands::UPDATE_CURSOR,
        SqlStatement::UpdateSearched { .. } => commands::UPDATE_WHERE,
        SqlStatement::SetSetting { .. } => commands::SET_SETTING,
        SqlStatement::SetDatabase { .. } => commands::SET_DATABASE,
        SqlStatement::Index(i) => index_command(i),
        SqlStatement::User(u) => user_command(u),
        SqlStatement::Served(ServedStatement::End { .. }) => commands::END_CONNECTION,
        SqlStatement::Served(ServedStatement::Cancel { .. }) => commands::CANCEL_CONNECTION,
        SqlStatement::Call(_) => commands::CALL,
        SqlStatement::Return(_) => commands::RETURN,
    }
}

/// How COMMAND_FUNCTION names the connection statement `c` [ISO/IEC
/// 9075-2:2023 §23.1 Table 39].
fn connection_command(c: &ConnectionStatement) -> Command {
    match c {
        ConnectionStatement::Connect { .. } | ConnectionStatement::ConnectDefault => commands::CONNECT,
        ConnectionStatement::Set(_) => commands::SET_CONNECTION,
        ConnectionStatement::Disconnect(_) => commands::DISCONNECT,
    }
}

/// \[EXTENSION\] How COMMAND_FUNCTION names the index statement `i`.
fn index_command(i: &IndexStatement) -> Command {
    match i {
        IndexStatement::Create { .. } => commands::CREATE_INDEX,
        IndexStatement::Drop { .. } => commands::DROP_INDEX,
        IndexStatement::Alter { .. } | IndexStatement::Maintain { .. } => commands::ALTER_INDEX,
    }
}

/// \[EXTENSION\] How COMMAND_FUNCTION names the user statement `u`.
fn user_command(u: &UserStatement) -> Command {
    match u {
        UserStatement::Create { .. } => commands::CREATE_USER,
        UserStatement::Alter { .. } | UserStatement::RoleCreator { .. } => commands::ALTER_USER,
        UserStatement::Drop { .. } => commands::DROP_USER,
        UserStatement::Owner { .. } | UserStatement::Members { .. } => commands::ALTER_DATABASE,
    }
}
