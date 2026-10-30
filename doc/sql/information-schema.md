# The Information Schema

[Contents](README.md)

The schema INFORMATION_SCHEMA describes the database in tables: its
schemas, tables, columns, constraints and privileges, and, of turborel's
own, its indexes, settings and users. Every user may read it, and may
grant SELECT on views of it. It is read like any table, in the
session's transaction, as of its snapshot; it is never written.

Each view shows only what the session's user may see: an object it
owns, or holds a privilege on, or that PUBLIC does.

```sql
SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ANN';
```

Run by BOB, who holds no privilege on ANN's tables, it gives no row;
once ANN grants BOB SELECT on DEPT, it gives DEPT.

Its names are of the types SQL_IDENTIFIER, CHARACTER VARYING(128), and
its texts of CHARACTER_DATA, CHARACTER VARYING(32767); a text longer than
that, as a long view's definition, is shown null. The catalog is named
TURBOREL, unless the setting CATALOG_NAME names it otherwise.

## The views

The views of SQL-92, each with the columns the standard gives it:

| View | One row of each |
|---|---|
| INFORMATION_SCHEMA_CATALOG_NAME | the catalog: its name |
| SCHEMATA | schema the user owns |
| TABLES | table and view, its TABLE_TYPE BASE TABLE, VIEW, GLOBAL TEMPORARY or LOCAL TEMPORARY |
| VIEWS | view: its definition, CHECK OPTION, whether updatable |
| COLUMNS | column of a table or view: its position, type, default, nullability, domain |
| DOMAINS | domain: its type and default |
| DOMAIN_CONSTRAINTS | constraint of a domain |
| TABLE_CONSTRAINTS | constraint of a table: its kind, PRIMARY KEY, UNIQUE, FOREIGN KEY or CHECK, and whether deferrable |
| REFERENTIAL_CONSTRAINTS | foreign key: the constraint it references, its MATCH and actions |
| CHECK_CONSTRAINTS | CHECK, domain constraint and assertion: its condition |
| KEY_COLUMN_USAGE | column of a UNIQUE, PRIMARY KEY or foreign key, in order |
| ASSERTIONS | assertion: whether deferrable |
| TABLE_PRIVILEGES | privilege on a table |
| COLUMN_PRIVILEGES | privilege on a column |
| USAGE_PRIVILEGES | USAGE on a domain, a character set or a collation |
| VIEW_TABLE_USAGE | table a view reads |
| VIEW_COLUMN_USAGE | column a view reads |
| CONSTRAINT_TABLE_USAGE | table a constraint refers to |
| CONSTRAINT_COLUMN_USAGE | column a constraint refers to |
| COLUMN_DOMAIN_USAGE | column of a domain |
| CHARACTER_SETS | character set: SQL_TEXT, the one there is |
| COLLATIONS | collation: SQL_TEXT's, the one there is |
| TRANSLATIONS | translation: none |
| SQL_LANGUAGES | standard conformance is claimed to: ISO 9075 of 1987, SQL-86, at Level 2, by direct invocation |

NOT NULL is shown as a CHECK constraint, `C IS NOT NULL`. A constraint
given no name has one made for it: the table's name, its kind and a
number, as `EMPLOYEES_PRIMARY1`. A temporary table the session declares
is in no view.

The types, in COLUMNS and DOMAINS: a number's by the name it was
declared with, INTEGER, SMALLINT, DECIMAL or NUMERIC, of its precision
and scale, radix 10, or REAL, DOUBLE PRECISION or FLOAT, of the binary
precision it holds values of, 24 or 53, radix 2; a national character
string as CHARACTER, its octet length four octets a character.

## This implementation's own views

| View | Columns | One row of each |
|---|---|---|
| TABLE_OPTIONS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ENGINE_NAME, OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, CHANGED_BY, DESCRIPTION | option of a table's storage engine, its value set or its default |
| INDEXES | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, METHOD_NAME, IS_UNIQUE, COLUMN_NAMES | index, a UNIQUE or PRIMARY KEY constraint's among them |
| INDEX_OPTIONS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, METHOD_NAME, OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, CHANGED_BY, DESCRIPTION | option of an index |
| SETTINGS | SETTING_NAME, SETTING_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, IS_SESSION_SETTABLE, TAKES_EFFECT, DESCRIPTION | setting, its value for the session |
| USERS | USER_NAME, IS_OWNER | user: to the owner every one, to another user itself |
| MEMORY_RESERVATIONS | RESERVED_BY, BYTES | part of the memory budget reserved, as a buffer pool's |
| JOBS | LABEL, STATE, THREADED, RUNS, RESTARTS, LAST_ERROR | job of the database's background work, `turborel-checkpoint` or `turborel-maintenance`: IDLE, DUE or RUNNING; whether a thread of its own runs it; how often it ran, and failed by a fault; its last error, or NULL |

VALUE_SOURCE says where a value comes from: DEFAULT, or the place it was
set. [SET](statements/set-setting.md) and
[SET DATABASE](statements/set-database.md) set the settings;
[Settings](../appendix/c-settings.md) lists them.

```sql
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE, IS_SESSION_SETTABLE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME IN ('LOCK_WAIT', 'MEMORY_BUDGET', 'TIME_ZONE');
SELECT * FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS;
```

```
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE | IS_SESSION_SETTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------+---------------------
 LOCK_WAIT                                                    | NO_WAIT                                                      | DEFAULT      | YES
 MEMORY_BUDGET                                                | 268435456                                                    | DEFAULT      | NO
 TIME_ZONE                                                    | +00:00                                                       | DEFAULT      | YES
(3 rows)
 RESERVED_BY                                                  |                BYTES
--------------------------------------------------------------+----------------------
 the buffer pool of the data file                             |             33554432
(1 row)
```

The second, of a database kept in a directory: one in memory has no
buffer pool. MEMORY_RESERVATIONS and JOBS show their rows to the
database's owner alone; JOBS.POLICY says how the jobs JOBS lists
are run ([Settings](../appendix/c-settings.md)).

The standard: ISO/IEC 9075:1992 §21.1, §21.2.
