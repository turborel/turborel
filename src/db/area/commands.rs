//! What the diagnostics area calls each statement: COMMAND_FUNCTION's and
//! DYNAMIC_FUNCTION's identifier [ISO/IEC 9075:1992 §18.1 GR1c, GR1d,
//! Table 22], and COMMAND_FUNCTION_CODE's and DYNAMIC_FUNCTION_CODE's code
//! [ISO/IEC 9075-2:2023 §23.1 GR2d, GR2e, Table 39].
//!
//! Each identifier is SQL-92's, where Table 22 gives one, and each code is
//! SQL:2023's, so a code tells apart statements one identifier names: the
//! three SELECTs, and a positioned DELETE or UPDATE prepared from one of a
//! dynamic cursor. A statement Table 22 has no row for takes Table 39's
//! identifier. A statement of this implementation takes its first words
//! and a negative code of its own (IE005, IV190, IV212).

use crate::ast::{ControlStatement, Iteration};

/// A statement, as the diagnostics area names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::db) struct Command {
    /// COMMAND_FUNCTION's or DYNAMIC_FUNCTION's value.
    pub(super) identifier: &'static str,
    /// COMMAND_FUNCTION_CODE's or DYNAMIC_FUNCTION_CODE's value.
    pub(super) code: i32,
}

impl Default for Command {
    fn default() -> Command {
        UNRECOGNIZED
    }
}

const fn command(identifier: &'static str, code: i32) -> Command {
    Command { identifier, code }
}

// [ISO/IEC 9075-2:2023 §23.1 Table 39] The standard's statements.

/// `<allocate extended dynamic cursor statement>`.
pub(in crate::db) const ALLOCATE_CURSOR: Command = command("ALLOCATE CURSOR", 1);
/// `<allocate descriptor statement>`.
pub(in crate::db) const ALLOCATE_DESCRIPTOR: Command = command("ALLOCATE DESCRIPTOR", 2);
/// `<alter domain statement>`.
pub(in crate::db) const ALTER_DOMAIN: Command = command("ALTER DOMAIN", 3);
/// `<alter table statement>`.
pub(in crate::db) const ALTER_TABLE: Command = command("ALTER TABLE", 4);
/// `<trigger definition>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const CREATE_TRIGGER: Command = command("CREATE TRIGGER", 80);
/// `<assertion definition>`.
pub(in crate::db) const CREATE_ASSERTION: Command = command("CREATE ASSERTION", 6);
/// `<call statement>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const CALL: Command = command("CALL", 7);
/// `<close statement>`.
pub(in crate::db) const CLOSE_CURSOR: Command = command("CLOSE CURSOR", 9);
/// `<collation definition>`.
pub(in crate::db) const CREATE_COLLATION: Command = command("CREATE COLLATION", 10);
/// `<free locator statement>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const FREE_LOCATOR: Command = command("FREE LOCATOR", 98);
/// `<hold locator statement>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const HOLD_LOCATOR: Command = command("HOLD LOCATOR", 99);
/// `<commit statement>`.
pub(in crate::db) const COMMIT_WORK: Command = command("COMMIT WORK", 11);
/// `<connect statement>`.
pub(in crate::db) const CONNECT: Command = command("CONNECT", 13);
/// `<deallocate descriptor statement>`.
pub(in crate::db) const DEALLOCATE_DESCRIPTOR: Command = command("DEALLOCATE DESCRIPTOR", 15);
/// `<deallocate prepared statement>`.
pub(in crate::db) const DEALLOCATE_PREPARE: Command = command("DEALLOCATE PREPARE", 16);
/// `<delete statement: positioned>`.
pub(in crate::db) const DELETE_CURSOR: Command = command("DELETE CURSOR", 18);
/// `<delete statement: searched>`.
pub(in crate::db) const DELETE_WHERE: Command = command("DELETE WHERE", 19);
/// `<describe statement>`.
pub(in crate::db) const DESCRIBE: Command = command("DESCRIBE", 20);
/// `<direct select statement: multiple rows>`.
pub(in crate::db) const SELECT_ROWS: Command = command("SELECT", 21);
/// `<disconnect statement>`.
pub(in crate::db) const DISCONNECT: Command = command("DISCONNECT", 22);
/// `<domain definition>`.
pub(in crate::db) const CREATE_DOMAIN: Command = command("CREATE DOMAIN", 23);
/// `<drop assertion statement>`.
pub(in crate::db) const DROP_ASSERTION: Command = command("DROP ASSERTION", 24);
/// `<drop collation statement>`.
pub(in crate::db) const DROP_COLLATION: Command = command("DROP COLLATION", 26);
/// `<drop role statement>`.
pub(in crate::db) const DROP_ROLE: Command = command("DROP ROLE", 29);
/// `<drop routine statement>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const DROP_ROUTINE: Command = command("DROP ROUTINE", 30);
/// `<drop module statement>` [ISO/IEC 9075-4:2006 draft §17.1 Table 2].
pub(in crate::db) const DROP_MODULE: Command = command("DROP MODULE", 28);
/// `<SQL-server module definition>` [ISO/IEC 9075-4:2006 draft §17.1 Table 2].
pub(in crate::db) const CREATE_MODULE: Command = command("CREATE MODULE", 51);
/// `<schema routine>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const CREATE_ROUTINE: Command = command("CREATE ROUTINE", 14);
/// `<return statement>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const RETURN: Command = command("RETURN", 58);
/// `<drop trigger statement>` [ISO/IEC 9075-2:2023 Table 39].
pub(in crate::db) const DROP_TRIGGER: Command = command("DROP TRIGGER", 34);
/// `<drop domain statement>`.
pub(in crate::db) const DROP_DOMAIN: Command = command("DROP DOMAIN", 27);
/// `<drop schema statement>`.
pub(in crate::db) const DROP_SCHEMA: Command = command("DROP SCHEMA", 31);
/// `<drop table statement>`.
pub(in crate::db) const DROP_TABLE: Command = command("DROP TABLE", 32);
/// `<drop data type statement>`.
pub(in crate::db) const DROP_TYPE: Command = command("DROP TYPE", 35);
/// `<drop view statement>`.
pub(in crate::db) const DROP_VIEW: Command = command("DROP VIEW", 36);
/// `<dynamic close statement>`.
pub(in crate::db) const DYNAMIC_CLOSE: Command = command("DYNAMIC CLOSE", 37);
/// `<dynamic delete statement: positioned>`.
pub(in crate::db) const DYNAMIC_DELETE_CURSOR: Command = command("DYNAMIC DELETE CURSOR", 38);
/// `<dynamic fetch statement>`.
pub(in crate::db) const DYNAMIC_FETCH: Command = command("DYNAMIC FETCH", 39);
/// `<dynamic open statement>`.
pub(in crate::db) const DYNAMIC_OPEN: Command = command("DYNAMIC OPEN", 40);
/// `<dynamic single row select statement>`.
pub(in crate::db) const DYNAMIC_SELECT: Command = command("SELECT", 41);
/// `<dynamic update statement: positioned>`.
pub(in crate::db) const DYNAMIC_UPDATE_CURSOR: Command = command("DYNAMIC UPDATE CURSOR", 42);
/// `<execute immediate statement>`.
pub(in crate::db) const EXECUTE_IMMEDIATE: Command = command("EXECUTE IMMEDIATE", 43);
/// `<execute statement>`.
pub(in crate::db) const EXECUTE: Command = command("EXECUTE", 44);
/// `<fetch statement>`.
pub(in crate::db) const FETCH: Command = command("FETCH", 45);
/// `<get descriptor statement>`.
pub(in crate::db) const GET_DESCRIPTOR: Command = command("GET DESCRIPTOR", 47);
/// `<grant privilege statement>`.
pub(in crate::db) const GRANT: Command = command("GRANT", 48);
/// `<grant role statement>`.
pub(in crate::db) const GRANT_ROLE: Command = command("GRANT ROLE", 49);
/// `<insert statement>`.
pub(in crate::db) const INSERT: Command = command("INSERT", 50);
/// `<open statement>`.
pub(in crate::db) const OPEN: Command = command("OPEN", 53);
/// `<preparable dynamic delete statement: positioned>`, which Table 22
/// names as the dynamic one.
pub(in crate::db) const PREPARABLE_DELETE_CURSOR: Command = command("DYNAMIC DELETE CURSOR", 54);
/// `<preparable dynamic update statement: positioned>`, which Table 22
/// names as the dynamic one.
pub(in crate::db) const PREPARABLE_UPDATE_CURSOR: Command = command("DYNAMIC UPDATE CURSOR", 55);
/// `<prepare statement>`.
pub(in crate::db) const PREPARE: Command = command("PREPARE", 56);
/// `<revoke privilege statement>`.
pub(in crate::db) const REVOKE: Command = command("REVOKE", 59);
/// `<role definition>`.
pub(in crate::db) const CREATE_ROLE: Command = command("CREATE ROLE", 61);
/// `<revoke role statement>`.
pub(in crate::db) const REVOKE_ROLE: Command = command("REVOKE ROLE", 129);
/// `<rollback statement>`.
pub(in crate::db) const ROLLBACK_WORK: Command = command("ROLLBACK WORK", 62);
/// `<release savepoint statement>`.
pub(in crate::db) const RELEASE_SAVEPOINT: Command = command("RELEASE SAVEPOINT", 57);
/// `<savepoint statement>`.
pub(in crate::db) const SAVEPOINT: Command = command("SAVEPOINT", 63);
/// `<schema definition>`.
pub(in crate::db) const CREATE_SCHEMA: Command = command("CREATE SCHEMA", 64);
/// `<select statement: single row>`.
pub(in crate::db) const SELECT_ROW: Command = command("SELECT", 65);
/// `<set catalog statement>`.
pub(in crate::db) const SET_CATALOG: Command = command("SET CATALOG", 66);
/// `<set connection statement>`.
pub(in crate::db) const SET_CONNECTION: Command = command("SET CONNECTION", 67);
/// `<set constraints mode statement>`.
pub(in crate::db) const SET_CONSTRAINT: Command = command("SET CONSTRAINT", 68);
/// `<set descriptor statement>`.
pub(in crate::db) const SET_DESCRIPTOR: Command = command("SET DESCRIPTOR", 70);
/// `<set local time zone statement>`.
pub(in crate::db) const SET_TIME_ZONE: Command = command("SET TIME ZONE", 71);
/// `<set role statement>`.
pub(in crate::db) const SET_ROLE: Command = command("SET ROLE", 73);
/// `<set schema statement>`.
pub(in crate::db) const SET_SCHEMA: Command = command("SET SCHEMA", 74);
/// `<set transaction statement>`.
pub(in crate::db) const SET_TRANSACTION: Command = command("SET TRANSACTION", 75);
/// `<set session user identifier statement>`.
pub(in crate::db) const SET_SESSION_AUTHORIZATION: Command = command("SET SESSION AUTHORIZATION", 76);
/// `<table definition>`.
pub(in crate::db) const CREATE_TABLE: Command = command("CREATE TABLE", 77);
/// `<update statement: positioned>`.
pub(in crate::db) const UPDATE_CURSOR: Command = command("UPDATE CURSOR", 81);
/// `<update statement: searched>`.
pub(in crate::db) const UPDATE_WHERE: Command = command("UPDATE WHERE", 82);
/// `<user-defined type definition>`.
pub(in crate::db) const CREATE_TYPE: Command = command("CREATE TYPE", 83);
/// `<view definition>`.
pub(in crate::db) const CREATE_VIEW: Command = command("CREATE VIEW", 84);
/// `<dynamic select statement>`, which Table 22 has no row for.
pub(in crate::db) const SELECT_CURSOR: Command = command("SELECT CURSOR", 85);
/// `<merge statement>`.
pub(in crate::db) const MERGE: Command = command("MERGE", 128);
/// `<sequence generator definition>`.
pub(in crate::db) const CREATE_SEQUENCE: Command = command("CREATE SEQUENCE", 133);
/// `<alter sequence generator statement>`.
pub(in crate::db) const ALTER_SEQUENCE: Command = command("ALTER SEQUENCE", 134);
/// `<drop sequence generator statement>`.
pub(in crate::db) const DROP_SEQUENCE: Command = command("DROP SEQUENCE", 135);
/// `<set session collation statement>`.
pub(in crate::db) const SET_COLLATION: Command = command("SET COLLATION", 136);
/// `<truncate table statement>`.
pub(in crate::db) const TRUNCATE_TABLE: Command = command("TRUNCATE TABLE", 139);
/// `<user-defined cast definition>`.
pub(in crate::db) const CREATE_CAST: Command = command("CREATE CAST", 52);
/// `<alter type statement>`.
pub(in crate::db) const ALTER_TYPE: Command = command("ALTER TYPE", 60);
/// `<drop user-defined cast statement>`.
pub(in crate::db) const DROP_CAST: Command = command("DROP CAST", 78);
/// `<user-defined ordering definition>`.
pub(in crate::db) const CREATE_ORDERING: Command = command("CREATE ORDERING", 114);
/// `<drop user-defined ordering statement>`.
pub(in crate::db) const DROP_ORDERING: Command = command("DROP ORDERING", 115);
/// `<drop transform statement>`.
pub(in crate::db) const DROP_TRANSFORM: Command = command("DROP TRANSFORM", 116);
/// `<transform definition>`.
pub(in crate::db) const CREATE_TRANSFORM: Command = command("CREATE TRANSFORM", 117);
/// `<set transform group statement>`.
pub(in crate::db) const SET_TRANSFORM_GROUP: Command = command("SET TRANSFORM GROUP", 118);
/// `<alter transform statement>`.
pub(in crate::db) const ALTER_TRANSFORM: Command = command("ALTER TRANSFORM", 127);

// [ISO/IEC 9075-4:2006 draft §17.1 GR3, Table 2] The SQL control
// statements, SIGNAL and RESIGNAL.

/// `<assignment statement>`.
pub(in crate::db) const ASSIGNMENT: Command = command("ASSIGNMENT", 5);
/// `<compound statement>`.
pub(in crate::db) const BEGIN_END: Command = command("BEGIN END", 12);
/// `<for statement>`.
pub(in crate::db) const FOR: Command = command("FOR", 46);
/// `<case statement>`.
pub(in crate::db) const CASE: Command = command("CASE", 86);
/// `<if statement>`.
pub(in crate::db) const IF: Command = command("IF", 88);
/// `<leave statement>`.
pub(in crate::db) const LEAVE: Command = command("LEAVE", 89);
/// `<loop statement>`.
pub(in crate::db) const LOOP: Command = command("LOOP", 90);
/// `<resignal statement>`.
pub(in crate::db) const RESIGNAL: Command = command("RESIGNAL", 91);
/// `<signal statement>`.
pub(in crate::db) const SIGNAL: Command = command("SIGNAL", 92);
/// `<repeat statement>`.
pub(in crate::db) const REPEAT: Command = command("REPEAT", 95);
/// `<while statement>`.
pub(in crate::db) const WHILE: Command = command("WHILE", 97);
/// `<iterate statement>`.
pub(in crate::db) const ITERATE: Command = command("ITERATE", 102);

/// [ISO/IEC 9075-4:2006 draft §17.2, §17.3] RESIGNAL if `resignal`, else
/// SIGNAL.
pub(in crate::db) fn signal(resignal: bool) -> Command {
    if resignal { RESIGNAL } else { SIGNAL }
}

/// [ISO/IEC 9075-4:2006 draft §17.1 Table 2] How the diagnostics area names
/// the SQL control statement `c`.
pub(in crate::db) fn control(c: &ControlStatement) -> Command {
    match c {
        ControlStatement::Assignment(_) => ASSIGNMENT,
        ControlStatement::Compound(_) => BEGIN_END,
        ControlStatement::Case(_) => CASE,
        ControlStatement::If(_) => IF,
        ControlStatement::Iterate(_) => ITERATE,
        ControlStatement::Leave(_) => LEAVE,
        ControlStatement::Iterated(l) => match l.iteration {
            Iteration::Loop => LOOP,
            Iteration::While(_) => WHILE,
            Iteration::Repeat(_) => REPEAT,
        },
        ControlStatement::For(_) => FOR,
    }
}

/// An unrecognized statement, one that did not parse; and the
/// DYNAMIC_FUNCTION of a statement that was not prepared.
pub(in crate::db) const UNRECOGNIZED: Command = command("", 0);

// [ISO/IEC 9075-2:2023 §23.1 GR2d, Table 39] This implementation's
// statements (IE005), each by its first words (IV190) and a negative code
// (IV212).

/// \[EXTENSION\] A `<temporary table declaration>`, run as a statement.
pub(in crate::db) const DECLARE_TABLE: Command = command("DECLARE LOCAL TEMPORARY TABLE", -1);
/// [ISO/IEC 9075:1992 §18.1 Table 22] GET DIAGNOSTICS, which Table 39 has
/// no row for. It never names itself, as it leaves the area as it was.
pub(in crate::db) const GET_DIAGNOSTICS: Command = command("GET DIAGNOSTICS", -2);
/// \[EXTENSION\] SET of a setting.
pub(in crate::db) const SET_SETTING: Command = command("SET", -3);
/// \[EXTENSION\] SET DATABASE.
pub(in crate::db) const SET_DATABASE: Command = command("SET DATABASE", -4);
/// \[EXTENSION\] CREATE INDEX.
pub(in crate::db) const CREATE_INDEX: Command = command("CREATE INDEX", -5);
/// \[EXTENSION\] ALTER INDEX.
pub(in crate::db) const ALTER_INDEX: Command = command("ALTER INDEX", -6);
/// \[EXTENSION\] DROP INDEX.
pub(in crate::db) const DROP_INDEX: Command = command("DROP INDEX", -7);
/// \[EXTENSION\] CREATE USER.
pub(in crate::db) const CREATE_USER: Command = command("CREATE USER", -8);
/// \[EXTENSION\] ALTER USER.
pub(in crate::db) const ALTER_USER: Command = command("ALTER USER", -9);
/// \[EXTENSION\] DROP USER.
pub(in crate::db) const DROP_USER: Command = command("DROP USER", -10);
/// \[EXTENSION\] ALTER DATABASE.
pub(in crate::db) const ALTER_DATABASE: Command = command("ALTER DATABASE", -11);
/// \[EXTENSION\] END CONNECTION.
pub(in crate::db) const END_CONNECTION: Command = command("END CONNECTION", -12);
/// \[EXTENSION\] CANCEL CONNECTION.
pub(in crate::db) const CANCEL_CONNECTION: Command = command("CANCEL CONNECTION", -13);
/// \[EXTENSION\] EXPLAIN.
pub(in crate::db) const EXPLAIN: Command = command("EXPLAIN", -14);
/// [ISO/IEC 9075-2:2023 §20.6] COPY DESCRIPTOR, which Table 39 has no row
/// for.
pub(in crate::db) const COPY_DESCRIPTOR: Command = command("COPY DESCRIPTOR", -15);
/// [ISO/IEC 9075-2:2023 §20.28] PIPE ROW, which Table 39 has no row for.
pub(in crate::db) const PIPE_ROW: Command = command("PIPE ROW", -16);
/// \[EXTENSION\] CREATE EMBEDDING MODEL.
pub(in crate::db) const CREATE_EMBEDDING_MODEL: Command = command("CREATE EMBEDDING MODEL", -17);
/// \[EXTENSION\] ALTER EMBEDDING MODEL.
pub(in crate::db) const ALTER_EMBEDDING_MODEL: Command = command("ALTER EMBEDDING MODEL", -18);
/// \[EXTENSION\] DROP EMBEDDING MODEL.
pub(in crate::db) const DROP_EMBEDDING_MODEL: Command = command("DROP EMBEDDING MODEL", -19);
/// \[EXTENSION\] CREATE VECTOR PIPELINE.
pub(in crate::db) const CREATE_VECTOR_PIPELINE: Command = command("CREATE VECTOR PIPELINE", -20);
/// \[EXTENSION\] DROP VECTOR PIPELINE.
pub(in crate::db) const DROP_VECTOR_PIPELINE: Command = command("DROP VECTOR PIPELINE", -21);
/// \[EXTENSION\] CREATE PROPERTY GRAPH (RULES.md D140): -27 to
/// -29 for property graphs, past the codes -17 to -21 of the vector
/// statements and -22 to -26 of XML's; the graph query language's take
/// -30 and on.
pub(in crate::db) const CREATE_PROPERTY_GRAPH: Command = command("CREATE PROPERTY GRAPH", -27);
/// \[EXTENSION\] ALTER PROPERTY GRAPH.
pub(in crate::db) const ALTER_PROPERTY_GRAPH: Command = command("ALTER PROPERTY GRAPH", -28);
/// \[EXTENSION\] DROP PROPERTY GRAPH.
pub(in crate::db) const DROP_PROPERTY_GRAPH: Command = command("DROP PROPERTY GRAPH", -29);
/// \[EXTENSION\] CREATE GRAPH TYPE (graph standard §4.3); GQL's codes
/// begin at -30, those before kept for the other graph statements.
pub(in crate::db) const CREATE_GRAPH_TYPE: Command = command("CREATE GRAPH TYPE", -30);
/// \[EXTENSION\] DROP GRAPH TYPE.
pub(in crate::db) const DROP_GRAPH_TYPE: Command = command("DROP GRAPH TYPE", -31);
/// \[EXTENSION\] CREATE GRAPH, of a GQL graph (§4.3).
pub(in crate::db) const CREATE_GRAPH: Command = command("CREATE GRAPH", -32);
/// \[EXTENSION\] DROP GRAPH, of a GQL graph.
pub(in crate::db) const DROP_GRAPH: Command = command("DROP GRAPH", -33);
/// \[EXTENSION\] EXECUTE GQL (§9).
pub(in crate::db) const EXECUTE_GQL: Command = command("EXECUTE GQL", -34);
/// \[EXTENSION\] CREATE XML SCHEMA [ISO/IEC 9075-14:2006 draft §4.2.6].
pub(in crate::db) const CREATE_XML_SCHEMA: Command = command("CREATE XML SCHEMA", -22);
/// \[EXTENSION\] DROP XML SCHEMA [ISO/IEC 9075-14:2006 draft §4.2.6].
pub(in crate::db) const DROP_XML_SCHEMA: Command = command("DROP XML SCHEMA", -23);
/// \[EXTENSION\] CREATE XQUERY MODULE (RULES.md D106).
pub(in crate::db) const CREATE_XQUERY_MODULE: Command = command("CREATE XQUERY MODULE", -24);
/// \[EXTENSION\] DROP XQUERY MODULE (RULES.md D106).
pub(in crate::db) const DROP_XQUERY_MODULE: Command = command("DROP XQUERY MODULE", -25);
/// \[EXTENSION\] ALTER XQUERY MODULE (RULES.md D106).
pub(in crate::db) const ALTER_XQUERY_MODULE: Command = command("ALTER XQUERY MODULE", -26);
/// \[EXTENSION\] BACKUP DATABASE (RULES.md D160): -35 to -37 for the
/// statements of backups, past GQL's.
pub(in crate::db) const BACKUP_DATABASE: Command = command("BACKUP DATABASE", -35);
/// \[EXTENSION\] CREATE RESTORE POINT (RULES.md D164).
pub(in crate::db) const CREATE_RESTORE_POINT: Command = command("CREATE RESTORE POINT", -36);
/// \[EXTENSION\] RESCAN BACKUP TARGET (RULES.md D166).
pub(in crate::db) const RESCAN_BACKUP_TARGET: Command = command("RESCAN BACKUP TARGET", -37);
/// \[EXTENSION\] CREATE EXTENSION (RULES.md D168).
pub(in crate::db) const CREATE_EXTENSION: Command = command("CREATE EXTENSION", -38);
/// \[EXTENSION\] ALTER EXTENSION (RULES.md D168).
pub(in crate::db) const ALTER_EXTENSION: Command = command("ALTER EXTENSION", -39);
/// \[EXTENSION\] DROP EXTENSION (RULES.md D168).
pub(in crate::db) const DROP_EXTENSION: Command = command("DROP EXTENSION", -40);
/// \[EXTENSION\] LOCK TABLE (RULES.md D186).
pub(in crate::db) const LOCK_TABLE: Command = command("LOCK TABLE", -42);
