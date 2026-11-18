# Statements

One reference page for each statement, in alphabetical order.

[Volume II: SQL Reference Manual](../README.md)

- [ALLOCATE CURSOR](allocate-cursor.md): allocate a cursor for a query prepared
- [ALLOCATE DESCRIPTOR](allocate-descriptor.md): allocate an SQL descriptor area
- [ALTER DATABASE](alter-database.md): hand the database on to another owner, or change the replica group's members
- [ALTER DOMAIN](alter-domain.md): change a domain's default or constraints
- [ALTER INDEX](alter-index.md): change an index's options
- [ALTER SEQUENCE](alter-sequence.md): change a sequence generator
- [ALTER TABLE](alter-table.md): change a base table
- [ALTER TRANSFORM](alter-transform.md): add or drop the functions of a transform group
- [ALTER TYPE](alter-type.md): add or drop a structured type's attributes and methods
- [ALTER USER](alter-user.md): change a user's password, or whether it may create roles
- [BEGIN](begin.md): group statements with their own variables, conditions, cursors and handlers
- [CALL](call.md): run an SQL-invoked procedure
- [CANCEL CONNECTION](cancel-connection.md): cancel what a connection of the server runs
- [CASE](case-statement.md): run the statements of the first WHEN that holds
- [CLOSE](close.md): close a cursor
- [COMMIT](commit.md): end a transaction, keeping its changes
- [CONNECT](connect.md): connect to a server
- [COPY DESCRIPTOR](copy-descriptor.md): copy a descriptor area, or an item's fields, to a PTF descriptor area *(in a component procedure)*
- [CREATE ASSERTION](create-assertion.md): define an assertion
- [CREATE CAST](create-cast.md): define a cast of a user-defined type by a function
- [CREATE COLLATION](create-collation.md): define a collation
- [CREATE DOMAIN](create-domain.md): define a domain
- [CREATE FUNCTION](create-function.md): define an SQL-invoked function, or a polymorphic table function
- [CREATE HYBRID VECTOR INDEX](create-hybrid-vector-index.md): define an index of a text and a vector column, for hybrid search
- [CREATE INDEX](create-index.md): define an index
- [CREATE VECTOR INDEX](create-vector-index.md): define an approximate nearest-neighbour index
- [CREATE METHOD](create-method.md): give a method of a user-defined type its body
- [CREATE MODULE](create-module.md): define an SQL-server module
- [CREATE ORDERING](create-ordering.md): say how values of a user-defined type compare
- [CREATE PROCEDURE](create-procedure.md): define an SQL-invoked procedure
- [CREATE ROLE](create-role.md): define a role
- [CREATE SCHEMA](create-schema.md): define a schema
- [CREATE SEQUENCE](create-sequence.md): define a sequence generator
- [CREATE TABLE](create-table.md): define a base table
- [CREATE TRANSFORM](create-transform.md): define transform groups of a user-defined type
- [CREATE TRIGGER](create-trigger.md): define a trigger
- [CREATE TYPE](create-type.md): define a distinct or structured type
- [CREATE USER](create-user.md): define a user
- [CREATE VIEW](create-view.md): define a viewed table
- [DECLARE CURSOR](declare-cursor.md): declare a cursor
- [DECLARE HANDLER](declare-handler.md): say what runs when a condition is raised *(in a compound statement)*
- [DEALLOCATE DESCRIPTOR](deallocate-descriptor.md): deallocate an SQL descriptor area
- [DEALLOCATE PREPARE](deallocate-prepare.md): forget a statement prepared
- [DECLARE LOCAL TEMPORARY TABLE](declare-local-temporary-table.md): declare a session's own temporary table
- [DELETE](delete.md): delete rows
- [DESCRIBE](describe.md): describe a statement prepared in an SQL descriptor area
- [DISCONNECT](disconnect.md): end connections
- [DROP ASSERTION](drop-assertion.md): drop an assertion
- [DROP CAST](drop-cast.md): drop a user-defined cast
- [DROP COLLATION](drop-collation.md): drop a collation
- [DROP DOMAIN](drop-domain.md): drop a domain
- [DROP INDEX](drop-index.md): drop an index
- [DROP MODULE](drop-module.md): drop an SQL-server module
- [DROP ORDERING](drop-ordering.md): drop a user-defined type's ordering
- [DROP ROLE](drop-role.md): drop a role
- [DROP ROUTINE](drop-routine.md): drop an SQL-invoked function, procedure or method
- [DROP SCHEMA](drop-schema.md): drop a schema
- [DROP SEQUENCE](drop-sequence.md): drop a sequence generator
- [DROP TABLE](drop-table.md): drop a base table
- [DROP TRANSFORM](drop-transform.md): drop transform groups of a user-defined type
- [DROP TRIGGER](drop-trigger.md): drop a trigger
- [DROP TYPE](drop-type.md): drop a distinct or structured type
- [DROP USER](drop-user.md): drop a user
- [DROP VIEW](drop-view.md): drop a view
- [END CONNECTION](end-connection.md): end a connection of the server
- [EXECUTE](execute.md): run a statement prepared
- [EXECUTE IMMEDIATE](execute-immediate.md): prepare a statement and run it at once
- [EXPLAIN](explain.md): show how a query is planned
- [FETCH](fetch.md): move a cursor and read its row
- [FOR](for.md): run statements for each row of a query
- [FREE LOCATOR](free-locator.md): end large object locators before their transaction does
- [GET DESCRIPTOR](get-descriptor.md): read fields of an SQL descriptor area
- [GET DIAGNOSTICS](get-diagnostics.md): read the diagnostics area
- [GRANT](grant.md): grant privileges, or roles
- [HOLD LOCATOR](hold-locator.md): keep large object locators past the end of their transaction
- [IF](if.md): run statements if a condition is true
- [INSERT](insert.md): insert rows
- [ITERATE](iterate.md): begin the next iteration of a loop *(in a labelled statement)*
- [LEAVE](leave.md): end a labelled statement
- [LOOP](loop.md): run statements again and again until something leaves
- [MERGE](merge.md): update, delete and insert rows of a table from one source
- [OPEN](open.md): open a cursor
- [PIPE ROW](pipe-row.md): give a row of a polymorphic table function's result *(in a component procedure)*
- [PREPARE](prepare.md): prepare a statement, by name
- [RELEASE SAVEPOINT](release-savepoint.md): destroy a savepoint, keeping the changes since
- [REPEAT](repeat.md): run statements until a condition is true
- [RESIGNAL](resignal.md): raise again the condition a handler handles *(in a handler)*
- [RETURN](return.md): give the value of an SQL-invoked function
- [REVOKE](revoke.md): revoke privileges, or roles
- [ROLLBACK](rollback.md): end a transaction, undoing its changes, or undo them back to a savepoint
- [SAVEPOINT](savepoint.md): mark a place in a transaction to roll back to
- [SELECT](select.md): read rows
- [SET](set-setting.md): change a session setting
- [SET (assignment)](set-variable.md): assign a value to an SQL variable or parameter *(in a compound statement)*
- [SET CATALOG](set-catalog.md): set the default catalog
- [SET CONNECTION](set-connection.md): choose the current connection
- [SET CONSTRAINTS](set-constraints.md): set constraint modes
- [SET DATABASE](set-database.md): change a database setting
- [SET DESCRIPTOR](set-descriptor.md): set fields of an SQL descriptor area
- [SET ROLE](set-role.md): set the session's current role
- [SET COLLATION](set-collation.md): set the session's collation
- [SET SCHEMA](set-schema.md): set the default schema
- [SET SESSION AUTHORIZATION](set-session-authorization.md): set the session's authorization identifier
- [SET TIME ZONE](set-time-zone.md): set the session's time zone
- [SET TRANSACTION](set-transaction.md): set the next transaction's modes
- [SET TRANSFORM GROUP](set-transform-group.md): choose the session's transform groups
- [SIGNAL](signal.md): raise a condition
- [TRUNCATE TABLE](truncate-table.md): delete every row of a table at once
- [UPDATE](update.md): update rows
- [WHILE](while.md): run statements while a condition is true

## Valid SQL not processed

These statements of SQL-92 are read in full, by their syntax, and then
refused as valid SQL that turborel does not process: SQLCODE -900,
SQLSTATE 0A000, the message naming what is not processed. A statement
written against its syntax is a syntax error instead (SQLCODE -102).

| Statements | Why |
|---|---|
| CREATE CHARACTER SET, CREATE TRANSLATION, DROP CHARACTER SET, DROP TRANSLATION | One character set, SQL_TEXT, its one collation, and no translations: never others |
| SET NAMES | The same |
| CONNECT, SET CONNECTION, DISCONNECT, run by a session of an embedded database | They are the SQL-agent's, run by the client, not by a database ([CONNECT](connect.md)) |

The standard: ISO/IEC 9075:1992 §11.28, §11.29, §11.32 and §11.33
(character sets and translations), §16.3 (SET NAMES) and §15.1–§15.3
(connections).
