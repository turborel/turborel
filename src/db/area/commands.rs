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
