# Glossary

[Contents](README.md)

## Terms

**assertion**: a constraint of a schema, on no one table; its
condition is checked after each statement that changes a table it
reads. [CREATE ASSERTION](../sql/statements/create-assertion.md).

**authorization identifier**: the name a session acts as, and that
owns schemas and holds privileges: a user's, of a server's client.

**base table**: a table that holds rows, as CREATE TABLE makes one; not
a view.

**catalog**: the database's schemas together, named by CATALOG.NAME.

**checkpoint**: a point at which a database kept in a directory has
written its changes to its data file, so recovery reads the log only
from there.

**column**: a value of each row of a table, of a name and a data type.

**constraint**: a rule the rows of tables must keep: NOT NULL, UNIQUE,
PRIMARY KEY, a foreign key, a CHECK, a domain's or an assertion.

**correlation name**: a name a FROM clause gives a table, `STAFF S`.

**cursor**: a query read one row at a time, in a program.
[DECLARE CURSOR](../sql/statements/declare-cursor.md).

**deferred**: of a constraint, checked at COMMIT rather than at the end
of each statement.

**diagnostics area**: where a statement's outcome is kept, for GET
DIAGNOSTICS.

**direct SQL**: statements run on their own, as the client runs them,
not in a program's procedures.

**domain**: a data type with a default and constraints, that columns
may be of. [CREATE DOMAIN](../sql/statements/create-domain.md).

**engine**: what keeps a table's rows: the row store, `rowstore`, the
default; the column store, `columnstore`, named by CREATE TABLE ...
USING; or one a plug-in registers.
[Table Options](g-table-options.md).

**extension**: what turborel has that the standards do not, marked
*(extension)* in this manual.

**foreign key**: a constraint that a row's values match those of a row
of another table, or its own.

**index**: a structure that finds rows by the values of columns, kept
by an index method: a B-link tree or a hash table.

**Information Schema**: the schema INFORMATION_SCHEMA, of views that
describe the database. [The Information Schema](../sql/information-schema.md).

**isolation level**: how much a transaction sees of others running at
once. [SET TRANSACTION](../sql/statements/set-transaction.md).

**log**: the write-ahead log of a database kept in a directory: each
change written there before it is made, so a crash loses no committed
transaction.

**null value**: no value: of a column not given one, or a set function
of no rows.

**owner**: of a schema, the authorization identifier it is of; of the
database, the user who manages its users and settings.

**plug-in**: code a program or the server registers with the database:
a storage engine, an index method, a data type, a function, and more.

**privilege**: the right to do an action on an object: SELECT, INSERT,
UPDATE, DELETE, REFERENCES or USAGE. [GRANT](../sql/statements/grant.md).

**schema**: a named set of tables, views, domains and assertions, of
one owner. [CREATE SCHEMA](../sql/statements/create-schema.md).

**session**: a connection's, or a program's, run of statements, of an
authorization identifier, a default schema, a time zone and settings
of its own.

**setting**: a named value that changes how the database or a session
works. [Settings](c-settings.md).

**SQLSTATE**: the five characters of a statement's outcome.
[SQLSTATE and SQLCODE](a-sqlstate.md).

**temporary table**: a table of which each session has rows of its own,
gone as the session ends; global, created local or declared local.

**transaction**: statements whose changes are made together, or not at
all: begun by the first statement, ended by COMMIT or ROLLBACK.

**unknown**: the truth of a comparison with a null, neither true nor
false.

**user**: a name a server's client may prove it is, by a password the
database keeps a verifier of. [CREATE USER](../sql/statements/create-user.md).

**view**: a table whose rows are those of its query, found each time it
is read. [CREATE VIEW](../sql/statements/create-view.md).
