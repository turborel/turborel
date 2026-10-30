# Statements

One reference page for each statement, in alphabetical order.

[Volume II: SQL Reference Manual](../README.md)

- [ALTER DOMAIN](alter-domain.md): change a domain's default or constraints
- [ALTER INDEX](alter-index.md): change an index's options
- [ALTER TABLE](alter-table.md): change a base table
- [ALTER USER](alter-user.md): change a user's password
- [CANCEL CONNECTION](cancel-connection.md): cancel what a connection of the server runs
- [CLOSE](close.md): close a cursor
- [COMMIT](commit.md): end a transaction, keeping its changes
- [CONNECT](connect.md): connect to a server
- [CREATE ASSERTION](create-assertion.md): define an assertion
- [CREATE DOMAIN](create-domain.md): define a domain
- [CREATE INDEX](create-index.md): define an index
- [CREATE SCHEMA](create-schema.md): define a schema
- [CREATE TABLE](create-table.md): define a base table
- [CREATE USER](create-user.md): define a user
- [CREATE VIEW](create-view.md): define a viewed table
- [DECLARE CURSOR](declare-cursor.md): declare a cursor
- [DECLARE LOCAL TEMPORARY TABLE](declare-local-temporary-table.md): declare a session's own temporary table
- [DELETE](delete.md): delete rows
- [DISCONNECT](disconnect.md): end connections
- [DROP ASSERTION](drop-assertion.md): drop an assertion
- [DROP DOMAIN](drop-domain.md): drop a domain
- [DROP INDEX](drop-index.md): drop an index
- [DROP SCHEMA](drop-schema.md): drop a schema
- [DROP TABLE](drop-table.md): drop a base table
- [DROP USER](drop-user.md): drop a user
- [DROP VIEW](drop-view.md): drop a view
- [END CONNECTION](end-connection.md): end a connection of the server
- [FETCH](fetch.md): move a cursor and read its row
- [GET DIAGNOSTICS](get-diagnostics.md): read the diagnostics area
- [GRANT](grant.md): grant privileges
- [INSERT](insert.md): insert rows
- [OPEN](open.md): open a cursor
- [REVOKE](revoke.md): revoke privileges
- [ROLLBACK](rollback.md): end a transaction, undoing its changes
- [SELECT](select.md): read rows
- [SET](set-setting.md): change a session setting
- [SET CATALOG](set-catalog.md): set the default catalog
- [SET CONNECTION](set-connection.md): choose the current connection
- [SET CONSTRAINTS](set-constraints.md): set constraint modes
- [SET DATABASE](set-database.md): change a database setting
- [SET SCHEMA](set-schema.md): set the default schema
- [SET SESSION AUTHORIZATION](set-session-authorization.md): set the session's authorization identifier
- [SET TIME ZONE](set-time-zone.md): set the session's time zone
- [SET TRANSACTION](set-transaction.md): set the next transaction's modes
- [UPDATE](update.md): update rows

## Valid SQL not processed

These statements of SQL-92 are read in full, by their syntax, and then
refused as valid SQL that turborel does not process: SQLCODE -900,
SQLSTATE 0A000, the message naming what is not processed. A statement
written against its syntax is a syntax error instead (SQLCODE -102).

| Statements | Why |
|---|---|
| ALLOCATE DESCRIPTOR, DEALLOCATE DESCRIPTOR, GET DESCRIPTOR, SET DESCRIPTOR | Dynamic SQL in SQL text: a program prepares statements through the [client library](../../client/02-library.md) or the [embedded library](../../client/03-embedding.md), and their parameters and results are described there |
| PREPARE, DEALLOCATE PREPARE, DESCRIBE, EXECUTE, EXECUTE IMMEDIATE | The same |
| ALLOCATE CURSOR; OPEN, FETCH and CLOSE of a dynamic cursor; DELETE and UPDATE WHERE CURRENT OF a dynamic cursor | The same |
| CREATE CHARACTER SET, CREATE COLLATION, CREATE TRANSLATION, DROP CHARACTER SET, DROP COLLATION, DROP TRANSLATION | One character set, SQL_TEXT, its one collation, and no translations: never others |
| SET NAMES | The same |
| CONNECT, SET CONNECTION, DISCONNECT, run by a session of an embedded database | They are the SQL-agent's, run by the client, not by a database ([CONNECT](connect.md)) |

The standard: ISO/IEC 9075:1992 §17.2–§17.20 (dynamic SQL), §11.28–§11.33
(character sets, collations and translations), §16.3 (SET NAMES) and
§15.1–§15.3 (connections).
