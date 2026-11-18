# The Information Schema

[Contents](README.md)

The schema INFORMATION_SCHEMA describes the database in tables: its
schemas, tables, columns, constraints, privileges, roles and triggers,
and, of turborel's
own, its indexes, settings and users. Every user may read it, and may
grant SELECT on views of it. It is read like any table, in the
session's transaction, as of its snapshot; it is never written.

Each view shows only what the session's user may see: an object it
owns, or holds a privilege on, or that PUBLIC does. Once the session
sets a role ([SET ROLE](statements/set-role.md)), each view shows too
what that role, and every role applicable for it, owns or holds a
privilege on, as the session's statements may use it.

```sql
SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ANN';
```

Run by BOB, who holds no privilege on ANN's tables, it gives no row;
once ANN grants BOB SELECT on DEPT, it gives DEPT.

Its names are of the types SQL_IDENTIFIER, CHARACTER VARYING(128), and
its texts of CHARACTER_DATA, CHARACTER VARYING(32767); a text longer than
that, as a long view's definition, is shown null. The catalog is named
TURBOREL, unless the setting CATALOG.NAME names it otherwise.

## The views

The views of SQL-92, each with the columns the standard gives it:

| View | One row of each |
|---|---|
| INFORMATION_SCHEMA_CATALOG_NAME | the catalog: its name |
| SCHEMATA | schema the user owns |
| TABLES | table and view, its TABLE_TYPE BASE TABLE, VIEW, GLOBAL TEMPORARY, LOCAL TEMPORARY or SYSTEM VERSIONED *(extension)* |
| VIEWS | view: its definition, CHECK OPTION, whether updatable |
| COLUMNS | column of a table or view: its position, type, default, nullability, domain, identity and generation |
| DOMAINS | domain: its type and default |
| DOMAIN_CONSTRAINTS | constraint of a domain |
| TABLE_CONSTRAINTS | constraint of a table: its kind, PRIMARY KEY, UNIQUE, FOREIGN KEY or CHECK, whether deferrable, and of a UNIQUE whether NULLS DISTINCT |
| REFERENTIAL_CONSTRAINTS | foreign key: the constraint it references, its MATCH and actions |
| CHECK_CONSTRAINTS | CHECK, domain constraint and assertion: its condition |
| KEY_COLUMN_USAGE | column of a UNIQUE, PRIMARY KEY or foreign key, in order |
| ASSERTIONS | assertion: whether deferrable |
| TABLE_PRIVILEGES | privilege on a table |
| COLUMN_PRIVILEGES | privilege on a column |
| USAGE_PRIVILEGES | USAGE on a domain, a character set, a collation or a sequence generator, its OBJECT_TYPE SEQUENCE |
| VIEW_TABLE_USAGE | table a view reads |
| VIEW_COLUMN_USAGE | column a view reads |
| CONSTRAINT_TABLE_USAGE | table a constraint refers to |
| CONSTRAINT_COLUMN_USAGE | column a constraint refers to |
| COLUMN_DOMAIN_USAGE | column of a domain |
| CHARACTER_SETS | character set: SQL_TEXT, the one there is |
| COLLATIONS | the built-in collations, and those CREATE COLLATION made |
| TRANSLATIONS | translation: none |
| SQL_LANGUAGES | standard conformance is claimed to: ISO 9075 of 1987, SQL-86, at Level 2, by direct invocation |

Two views of ISO/IEC 9075-11 describe the parts of array and row types
*(extension)*, of the tables and domains the views above show:

| View | One row of each |
|---|---|
| ELEMENT_TYPES | array or multiset type of a column, a domain, a distinct type's source, a function's parameter or result, or a part of one, with its OBJECT_TYPE TABLE, DOMAIN, USER-DEFINED TYPE or ROUTINE, COLLECTION_TYPE_IDENTIFIER the collection type's DTD_IDENTIFIER, and its element type, as COLUMNS gives a type |
| FIELDS | field of a row type of a column, a domain or a part of either, with ROW_IDENTIFIER the row type's DTD_IDENTIFIER, FIELD_NAME, ORDINAL_POSITION, and the field's type |

Six more of ISO/IEC 9075-11 describe user-defined types and the
functions a distinct type's definition makes ([CREATE TYPE](statements/create-type.md)),
and the functions and procedures CREATE FUNCTION and CREATE PROCEDURE
make, those the user, or PUBLIC, holds a privilege on:

| View | One row of each |
|---|---|
| USER_DEFINED_TYPES | structured type: USER_DEFINED_TYPE_CATEGORY STRUCTURED, IS_INSTANTIABLE and IS_FINAL as defined, its ordering's form, category and function, NONE if it has none, and its reference type's representation in REFERENCE_TYPE. Distinct type: USER_DEFINED_TYPE_CATEGORY DISTINCT, IS_INSTANTIABLE and IS_FINAL YES, ORDERING_FORM FULL and ORDERING_CATEGORY MAP with the specific name of its function to its source in ORDERING_ROUTINE_CATALOG, _SCHEMA and _NAME, or NONE of an array or multiset source, and the source type, as COLUMNS gives a type, its DTD_IDENTIFIER in SOURCE_DTD_IDENTIFIER |
| UDT_PRIVILEGES | privilege on a user-defined type, its PRIVILEGE_TYPE TYPE USAGE or UNDER |
| ROUTINES | routine: its specific name and its name, ROUTINE_TYPE FUNCTION or PROCEDURE, a function's result type, as COLUMNS gives a type, the type's name in TYPE_UDT_CATALOG, _SCHEMA and _NAME, null of a procedure, ROUTINE_BODY SQL, ROUTINE_DEFINITION its body to the owner of its schema and null to others, IS_DETERMINISTIC, SQL_DATA_ACCESS CONTAINS SQL, READS SQL DATA or MODIFIES SQL DATA, a function's IS_NULL_CALL, MODULE_CATALOG, MODULE_SCHEMA and MODULE_NAME the module of a module's routine, null of another, SCHEMA_LEVEL_ROUTINE YES, or NO of a module's routine, and SECURITY_TYPE DEFINER. A distinct type's function is DETERMINISTIC, CONTAINS SQL and not null-call. A polymorphic table function is a FUNCTION of a null result type |
| PARAMETERS | routine's parameter: its ORDINAL_POSITION, from 1, PARAMETER_MODE IN, OUT or INOUT, its name and its type. [EXTENSION] A polymorphic table function's generic table parameter is of DATA_TYPE TABLE, its descriptor parameter DESCRIPTOR, their other type columns null |
| ROUTINE_PRIVILEGES | privilege on a routine, its PRIVILEGE_TYPE EXECUTE, the owner's of _SYSTEM among them |
| TRANSFORMS | transform: a distinct type's own, its GROUP_NAME the name of the function from its source, of TRANSFORM_TYPE FROM SQL, its function to its source, and TO SQL, its function from it, and each function of a group CREATE TRANSFORM made |

Eight more describe structured types and typed tables
([CREATE TYPE](statements/create-type.md), [CREATE TABLE](statements/create-table.md)):

| View | One row of each |
|---|---|
| ATTRIBUTES | attribute of a structured type: its name, ORDINAL_POSITION, default and type, as COLUMNS gives a type |
| DIRECT_SUPERTYPES | structured type and its direct supertype |
| METHOD_SPECIFICATIONS | method specification: its specific name, its name, IS_STATIC, IS_OVERRIDING, IS_CONSTRUCTOR, its result type and characteristics |
| METHOD_SPECIFICATION_PARAMETERS | parameter of a method specification, as PARAMETERS gives one |
| DIRECT_SUPERTABLES | typed table or referenceable view and its direct supertable or superview |
| REFERENCED_TYPES | reference type of a column, attribute, parameter or part of one: its referenced type, as ELEMENT_TYPES gives a part |
| TABLE_METHOD_PRIVILEGES | privilege of SELECT on a method of a typed table's type, granted or held by the user or an enabled role |
| ROLE_TABLE_METHOD_GRANTS | such a privilege granted to or by an enabled role |

Eight more of ISO/IEC 9075-11 describe roles ([CREATE
ROLE](statements/create-role.md)) and the privileges granted to them:

| View | One row of each |
|---|---|
| ENABLED_ROLES | role the session has enabled: the role SET ROLE set, each role granted to it or to PUBLIC, and each granted to one of those in turn. None until a role is set |
| APPLICABLE_ROLES | grant of a role to the user, to PUBLIC, or to an enabled role, and of a role to one of those roles in turn, set or not: GRANTEE, ROLE_NAME, IS_GRANTABLE YES if granted WITH ADMIN OPTION |
| ADMINISTRABLE_ROLE_AUTHORIZATIONS | grant of a role the user may grant, revoke and drop, holding it WITH ADMIN OPTION through APPLICABLE_ROLES: GRANTEE, ROLE_NAME, IS_GRANTABLE |
| ROLE_TABLE_GRANTS | privilege on a table granted to an enabled role, or by one, with the columns of TABLE_PRIVILEGES, WITH_HIERARCHY among them |
| ROLE_COLUMN_GRANTS | such a privilege on a column, with the columns of COLUMN_PRIVILEGES |
| ROLE_USAGE_GRANTS | such USAGE, with the columns of USAGE_PRIVILEGES |
| ROLE_UDT_GRANTS | such a privilege on a user-defined type, with the columns of UDT_PRIVILEGES |
| ROLE_ROUTINE_GRANTS | such a privilege on a routine, with the columns of ROUTINE_PRIVILEGES |

A role's creator holds it WITH ADMIN OPTION by a grant from `_SYSTEM`,
which APPLICABLE_ROLES shows. ANN, the owner of table STAFF, has granted
SELECT on it to the role CLERK, CLERK to the role SENIOR, and SENIOR to
BOB WITH ADMIN OPTION. Run by BOB:

```sql
SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ANN';
SELECT ROLE_NAME FROM INFORMATION_SCHEMA.ENABLED_ROLES;
COMMIT;
SET ROLE 'SENIOR';
SELECT ROLE_NAME FROM INFORMATION_SCHEMA.ENABLED_ROLES ORDER BY 1;
SELECT GRANTEE, ROLE_NAME FROM INFORMATION_SCHEMA.APPLICABLE_ROLES ORDER BY 2;
SELECT GRANTEE, TABLE_NAME, PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.ROLE_TABLE_GRANTS;
SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ANN';
```

```
 TABLE_NAME
--------------------------------------------------------------
(0 rows)
 ROLE_NAME
--------------------------------------------------------------
(0 rows)
OK
OK
 ROLE_NAME
--------------------------------------------------------------
 CLERK
 SENIOR
(2 rows)
 GRANTEE                                                      | ROLE_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 SENIOR                                                       | CLERK
 BOB                                                          | SENIOR
(2 rows)
 GRANTEE                                                      | TABLE_NAME                                                   | PRIVILEGE_TYPE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 CLERK                                                        | STAFF                                                        | SELECT
(1 row)
 TABLE_NAME
--------------------------------------------------------------
 STAFF
(1 row)
```

Six more of ISO/IEC 9075-11 describe triggers ([CREATE
TRIGGER](statements/create-trigger.md)):

| View | One row of each |
|---|---|
| TRIGGERS | trigger on a table the user, or an enabled role, holds a privilege other than SELECT on: EVENT_MANIPULATION INSERT, DELETE or UPDATE, its subject table in EVENT_OBJECT_CATALOG, _SCHEMA and _TABLE, ACTION_ORDER its place, from 1, among the triggers of that table, event, time and orientation in the order they run, ACTION_CONDITION and ACTION_STATEMENT its WHEN condition and its action as written, to the owner of its schema and null to others, ACTION_ORIENTATION ROW or STATEMENT, ACTION_TIMING BEFORE, AFTER or INSTEAD OF, its transition names in ACTION_REFERENCE_OLD_TABLE, _NEW_TABLE, _OLD_ROW and _NEW_ROW, and CREATED, null, as no timestamp is kept |
| TRIGGERED_UPDATE_COLUMNS | column of the column list of an UPDATE OF trigger, of a column the user holds a privilege other than SELECT on |
| TRIGGER_TABLE_USAGE | table or view a trigger's action reads, of a trigger of a schema the user, or an enabled role, owns |
| TRIGGER_COLUMN_USAGE | column a trigger's action names, its transition variables' among them, of such a trigger |
| TRIGGER_ROUTINE_USAGE | function or procedure a trigger's action invokes, by its specific name, of such a trigger |
| TRIGGER_SEQUENCE_USAGE | sequence generator a trigger's action draws from, of such a trigger |

Run by ANN, the owner of these:

```sql
CREATE TABLE ORDERS (ID INTEGER, AMOUNT INTEGER);
CREATE TABLE AUDIT (N INTEGER, ID INTEGER);
CREATE SEQUENCE S;
CREATE TRIGGER LOGGED AFTER INSERT ON ORDERS REFERENCING NEW ROW AS R FOR EACH ROW WHEN (R.AMOUNT > 100) INSERT INTO AUDIT VALUES (NEXT VALUE FOR S, R.ID);
CREATE TRIGGER CHANGED AFTER UPDATE OF AMOUNT ON ORDERS FOR EACH STATEMENT DELETE FROM AUDIT;
COMMIT;
SELECT TRIGGER_NAME, EVENT_MANIPULATION, ACTION_ORIENTATION, ACTION_CONDITION FROM INFORMATION_SCHEMA.TRIGGERS ORDER BY 1;
SELECT TRIGGER_NAME, EVENT_OBJECT_COLUMN FROM INFORMATION_SCHEMA.TRIGGERED_UPDATE_COLUMNS;
SELECT TRIGGER_NAME, SEQUENCE_NAME FROM INFORMATION_SCHEMA.TRIGGER_SEQUENCE_USAGE;
```

```
OK
OK
OK
OK
OK
OK
 TRIGGER_NAME                                                 | EVENT_MANIPULATION                                           | ACTION_ORIENTATION                                           | ACTION_CONDITION
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 CHANGED                                                      | UPDATE                                                       | STATEMENT                                                    |
 LOGGED                                                       | INSERT                                                       | ROW                                                          | R.AMOUNT > 100
(2 rows)
 TRIGGER_NAME                                                 | EVENT_OBJECT_COLUMN
--------------------------------------------------------------+--------------------------------------------------------------
 CHANGED                                                      | AMOUNT
(1 row)
 TRIGGER_NAME                                                 | SEQUENCE_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 LOGGED                                                       | S
(1 row)
```

Five more of ISO/IEC 9075-4 describe SQL-server modules ([CREATE
MODULE](statements/create-module.md)):

| View | One row of each |
|---|---|
| MODULES | module the user, an enabled role or PUBLIC holds EXECUTE on: DEFAULT_CHARACTER_SET_CATALOG, _SCHEMA and _NAME SQL_TEXT's, DEFAULT_SCHEMA_CATALOG and _NAME its SCHEMA, MODULE_DEFINITION its text to the owner of its schema and null to others, MODULE_AUTHORIZATION null, SQL_PATH its PATH as written, null if none, and MODULE_CREATED and MODULE_LAST_ALTERED, null, as no timestamp is kept |
| MODULE_PRIVILEGES | privilege on a module, its PRIVILEGE_TYPE EXECUTE, the owner's of _SYSTEM among them, granted or granted by the user or an enabled role |
| ROLE_MODULE_GRANTS | such a privilege granted to or by an enabled role |
| MODULE_TABLE_USAGE | table or view a module's routine names, of a schema the user, or an enabled role, owns |
| MODULE_COLUMN_USAGE | column of such a table a module's routine names |

A module's declared temporary tables are in no view.

Run by ANN, the owner of these:

```sql
CREATE TABLE SALES (ID INTEGER, AMOUNT INTEGER);
CREATE MODULE SHOP
  PROCEDURE PLACE (IN ID INTEGER, IN AMOUNT INTEGER) MODIFIES SQL DATA INSERT INTO SALES VALUES (ID, AMOUNT);
  FUNCTION TOTAL () RETURNS INTEGER READS SQL DATA RETURN (SELECT SUM(AMOUNT) FROM SALES);
END MODULE;
GRANT EXECUTE ON MODULE SHOP TO PUBLIC;
COMMIT;
SELECT MODULE_NAME, DEFAULT_SCHEMA_NAME, SQL_PATH FROM INFORMATION_SCHEMA.MODULES;
SELECT GRANTOR, GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.MODULE_PRIVILEGES ORDER BY 1;
SELECT MODULE_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.MODULE_COLUMN_USAGE ORDER BY 3;
```

```
OK
OK
OK
OK
 MODULE_NAME                                                  | DEFAULT_SCHEMA_NAME                                          | SQL_PATH
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 SHOP                                                         | ANN                                                          | NULL
(1 row)
 GRANTOR                                                      | GRANTEE                                                      | PRIVILEGE_TYPE                                               | IS_GRANTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ANN                                                          | PUBLIC                                                       | EXECUTE                                                      | NO
 _SYSTEM                                                      | ANN                                                          | EXECUTE                                                      | YES
(2 rows)
 MODULE_NAME                                                  | TABLE_NAME                                                   | COLUMN_NAME
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 SHOP                                                         | SALES                                                        | AMOUNT
(1 row)
```

COLUMNS, ELEMENT_TYPES and FIELDS have three more columns, UDT_CATALOG,
UDT_SCHEMA and UDT_NAME, the name of a distinct type the column or part
is of, its DATA_TYPE then USER-DEFINED. They are null of another type.

COLUMNS has nine more columns *(extension)*, named as ISO/IEC 9075-11
names them. IS_IDENTITY is YES of an identity column. Of one,
IDENTITY_GENERATION is ALWAYS or BY DEFAULT, and IDENTITY_START,
IDENTITY_INCREMENT, IDENTITY_MAXIMUM and IDENTITY_MINIMUM are its
generator's options, each as text, and IDENTITY_CYCLE is YES if it cycles.
Of another column the six are null. IS_GENERATED is ALWAYS of a generated
column and NEVER of another, and GENERATION_EXPRESSION is a generated
column's expression as written. COLUMN_DEFAULT is null of both kinds.

COLUMNS has IS_HIDDEN last *(extension)*, YES of a pseudo-column of a
node or edge table, which `*` does not show, and NO of another
([CREATE TABLE](statements/create-table.md)).

COLUMNS and DOMAINS have two more *(extension)*, as ISO/IEC 9075-11
names them. DATA_TYPE is ARRAY, MULTISET or ROW of a column or domain
of such a type, MAXIMUM_CARDINALITY an array's maximum cardinality, and null of
another type. DTD_IDENTIFIER identifies the type's descriptor. It is a
column's ordinal position, or 1 of a domain, and null of a column of a domain,
whose type is the domain's. A part's is its parent's, a point, and the
field's place in its row, or 1 of an array's element, so the first field
of column 2 is `2.1` and the element of that field's array `2.1.1`.
ELEMENT_TYPES and FIELDS join to COLUMNS and DOMAINS by it.

TABLE_CONSTRAINTS has one more column *(extension)*, NULLS_DISTINCT, as
ISO/IEC 9075-11 names it: YES of a UNIQUE constraint of NULLS DISTINCT,
written or not, NO of one of NULLS NOT DISTINCT, and null of a PRIMARY
KEY and of every other kind.

NOT NULL is shown as a CHECK constraint, `C IS NOT NULL`. A constraint
given no name has one made for it: the table's name, its kind and a
number, as `EMPLOYEES_PRIMARY1`. A temporary table the session declares
is in no view.

The types, in COLUMNS and DOMAINS: a number's by the name it was
declared with, INTEGER, SMALLINT, BIGINT, DECIMAL or NUMERIC, of its precision
and scale, radix 10, or REAL, DOUBLE PRECISION or FLOAT, of the binary
precision it holds values of, 24 or 53, radix 2; a national character
string as CHARACTER, its octet length four octets a character.

## This implementation's own views

| View | Columns | One row of each |
|---|---|---|
| TABLE_OPTIONS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ENGINE_NAME, OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, CHANGED_BY, DESCRIPTION | option of a table's storage engine, its value set or its default |
| PERIODS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, PERIOD_NAME, START_COLUMN_NAME, END_COLUMN_NAME, CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME | period of a table the user holds a privilege on, SYSTEM_TIME or an application-time period: its start and end columns, and the period constraint of an application-time period ([CREATE TABLE](statements/create-table.md)) |
| INDEXES | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, METHOD_NAME, IS_UNIQUE, COLUMN_NAMES | index, a UNIQUE or PRIMARY KEY constraint's among them |
| SEQUENCES | SEQUENCE_CATALOG, SEQUENCE_SCHEMA, SEQUENCE_NAME, DATA_TYPE, NUMERIC_PRECISION, NUMERIC_PRECISION_RADIX, NUMERIC_SCALE, START_VALUE, MINIMUM_VALUE, MAXIMUM_VALUE, INCREMENT, CYCLE_OPTION | sequence generator the user holds USAGE on: its type, and its start, bounds and increment, each as text, and YES if it cycles ([CREATE SEQUENCE](statements/create-sequence.md)) |
| INDEX_OPTIONS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, METHOD_NAME, OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, CHANGED_BY, DESCRIPTION | option of an index |
| ADJACENCY_INDEXES | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, COLUMN_NAMES, SOURCE_COLUMNS, INCLUDED_COLUMNS, DIRECTION, CSR_BYTES | `adjacency` index: how many of its columns are the source, how many are included, which ways it keeps the edges, and the most bytes its compacted adjacency holds ([CREATE INDEX](statements/create-index.md)) |
| SETTINGS | SETTING_NAME, SETTING_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, IS_SESSION_SETTABLE, TAKES_EFFECT, DESCRIPTION, STARTED_VALUE, IS_PENDING | setting, its value for the session; of one taken at the next start, the value it started with, or null, and whether the value set waits for a restart |
| USERS | USER_NAME, IS_OWNER, CREATE_ROLE | user: to the owner every one, to another user itself; CREATE_ROLE YES if it may create a role |
| MEMORY_RESERVATIONS | RESERVED_BY, BYTES | part of the memory budget reserved, as a buffer pool's |
| CONNECTIONS | ID, USER_NAME, PEER, SECURE, STATE | connection a server serves: its user once authenticated, or NULL; where it came from, an address or `local`; whether it is secure; LOGIN, IDLE or RUNNING a statement |
| SERVER | WORKERS_BUSY, STATEMENTS_WAITING, CONNECTIONS, REFUSED, CLOSED_UNTOLD | one row: the workers running a statement, the statements waiting for one, the connections, and those refused past SERVER.MAX_CONNECTIONS, told why or closed at once |
| REPLICAS | MEMBER, ADDRESS, IS_SELF, ROLE, EPOCH, VIEW_NUMBER, IS_CONNECTED, LAST_HEARD_MS | member of the database's replica group ([Replication](../operator/09-replication.md)), in the epoch this node is in, to the owner: its number and address; whether it is this node; PRIMARY, BACKUP or UNKNOWN in the view this node knows; the epoch, and that view, or NULL; whether this node's link to it is up; the milliseconds since a frame from it arrived, or NULL |
| FILES | PATH, KIND, CLASS, IS_OPEN, READS, WRITES, SYNCS | file of the database: DATA, SHADOW (a side file of the data file, or of a plug-in's shadowed file), CATALOG, LOG, UNDO, SPILL, TRANSFER (a state transfer being fetched by a replica), PLUGIN (a plug-in's own), SERVER (its server's own: the diagnostics log, the socket, the TLS and key files) or OTHER; its class of durability, DURABLE (DATA, CATALOG, LOG, PLUGIN), DURABLE AT COMMIT (SHADOW), UNSYNCED (UNDO), SCRATCH (SPILL, TRANSFER), or null for a file not the database's; whether it is open now; its reads, writes and syncs |
| FILE_FORMATS | KIND, IS_SETTLED, BEHIND | kind of file of a database kept in a directory, DATA, LOG or CATALOG, to its owner: whether every one is in the formats STORAGE.ENCODINGS and STORAGE.CHECKSUM_KEY name; and what is still in others: the pages of the data file the job `turborel-rekey` has yet to look at, the log's segments, the catalog's file ([The files' formats](../operator/07-encryption.md#the-files-formats)) |
| GRAPH_ALGO_RUNS | RUN_ID, ALGORITHM, ARGUMENTS, USER_NAME, STARTED, ELAPSED_MICROS, VERTICES, EDGES, ROWS_RETURNED, THREADS, STATUS, MESSAGE | run of a function of GRAPH_ALGO *(extension)*, of the latest 1000, with its number from 1, its algorithm, its value arguments as `NAME => value`, who ran it, when it started and the microseconds it took, its graph's vertices and edges, its result's rows, the threads it ran on, DONE or FAILED, and a failed run's message or NULL; each user's own, every run of the database's owner ([Graph Algorithms](16-graph-algorithms.md)) |
| JOBS | LABEL, STATE, THREADED, RUNS, RESTARTS, LAST_ERROR | job of the database's background work, `turborel-checkpoint`, `turborel-maintenance`, `turborel-log-sync`, `turborel-read-ahead`, `turborel-assertion-totals`, `turborel-rekey`, `turborel-replica` or a plug-in's `turborel-<kind>`: IDLE, DUE or RUNNING; whether a thread of its own runs it; how often it ran, and failed by a fault; its last error, or NULL |
| ASSERTION_TOTALS | CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME, FORM_NUMBER, KIND, VALUE_COUNT, NULL_COUNT, TOTAL, ROWS_KEPT | total an assertion of a schema of the user's keeps, as committed: its form's number, SUM, AVG, COUNT or NULLS (of a set function's argument that may be null); the values counted, or NULL of NULLS; their sum, as text, or NULL of COUNT and NULLS; the nulls; and the rows keeping them, which the job `turborel-assertion-totals` folds into one |
| PROPERTY_GRAPHS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, PROPERTY_GRAPH_MODE, IS_INVOKER_RIGHTS, IS_VALID, PROPERTY_GRAPH_DEFINITION | property graph the user, an enabled role or PUBLIC holds SELECT on: ENFORCED or TRUSTED, YES if WITH INVOKER RIGHTS, NO if INVALID until compiled, and its definition as a CREATE PROPERTY GRAPH statement to the owner of its schema, null to others ([CREATE PROPERTY GRAPH](statements/create-property-graph.md)) |
| PG_ELEMENT_TABLES | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, ELEMENT_TABLE_KIND, TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION, IS_UNDIRECTED | element table of such a graph: VERTEX or EDGE, its table, its place among the graph's of its kind, and YES of an UNDIRECTED edge table |
| PG_LABELS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, LABEL_NAME, ORDINAL_POSITION | label of such an element table, in order |
| PG_PROPERTIES | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, LABEL_NAME, PROPERTY_NAME, ORDINAL_POSITION, PROPERTY_EXPRESSION, DATA_TYPE | property of such a label, in order: its expression as kept, and its declared type |
| PG_KEY_COLUMNS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, COLUMN_NAME, ORDINAL_POSITION | column of such an element table's key, in order |
| PG_EDGE_ENDPOINTS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, EDGE_TABLE_ALIAS, EDGE_END, VERTEX_TABLE_ALIAS, ORDINAL_POSITION, EDGE_COLUMN_NAME, VERTEX_COLUMN_NAME | column of such an edge table that joins it, at its SOURCE or DESTINATION end, to a vertex table, with the vertex table's column it matches |
| PROPERTY_GRAPH_PRIVILEGES | GRANTOR, GRANTEE, PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, PRIVILEGE_TYPE, IS_GRANTABLE | SELECT on a graph, the owner's of _SYSTEM among them, granted to or by the user, PUBLIC or an enabled role |
| GRAPH_TABLES | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, GRAPH_TABLE_TYPE | node or edge table the user holds a privilege on: NODE or EDGE ([CREATE TABLE](statements/create-table.md)) |
| CONNECTION_CONSTRAINTS | CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME, TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION, FROM_TABLE_CATALOG, FROM_TABLE_SCHEMA, FROM_TABLE_NAME, TO_TABLE_CATALOG, TO_TABLE_SCHEMA, TO_TABLE_NAME, DELETE_RULE | clause of a CONNECTION constraint of such an edge table, in order: the node tables its edges may join, and CASCADE or NO ACTION |
| VECTOR_COLUMNS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION, VECTOR_DIMENSIONS, VECTOR_FORMAT, VECTOR_LAYOUT | VECTOR column of a table the user holds a privilege on: the dimension count, format and layout its type fixes, NULL of one it leaves open *(extension)* ([Vectors](14-vectors.md)) |
| VECTOR_DISTANCE_FUNCTIONS | FUNCTION_CATALOG, FUNCTION_SCHEMA, FUNCTION_NAME, SPECIFIC_NAME, LANGUAGE, IS_DETERMINISTIC, SYMMETRIC, NON_NEGATIVE, TRIANGLE_INEQUALITY, FORMATS, FUNCTION_OWNER, DEFINITION | distance function the user holds EXECUTE on: its language, SQL; YES of each property it declares; the formats it measures, separated by commas; its owner; and its body, or NULL if longer than 4,096 characters *(extension)* ([CREATE VECTOR DISTANCE FUNCTION](statements/create-vector-distance-function.md)) |
| EMBEDDING_MODELS | MODEL_CATALOG, MODEL_SCHEMA, MODEL_NAME, MODEL_TYPE, DIMENSIONS, ELEMENT_FORMAT, LAYOUT, MODEL_VERSION, MODEL_FILE, IS_REGISTERED, MODEL_OWNER | embedding model the user owns or holds EXECUTE on: its plug-in; the dimension count, format and layout of its vectors; its version and file, or NULL; YES if its plug-in is registered; and its owner. Its options are not shown, as one may hold a secret *(extension)* ([CREATE EMBEDDING MODEL](statements/create-embedding-model.md)) |
| EMBEDDING_MODEL_PRIVILEGES | GRANTOR, GRANTEE, MODEL_CATALOG, MODEL_SCHEMA, MODEL_NAME, PRIVILEGE_TYPE, IS_GRANTABLE | grant of EXECUTE on an embedding model that the user made or holds, or PUBLIC holds, or of a model of a schema the user owns, as ROUTINE_PRIVILEGES shows a routine's; the owner's own granted by _SYSTEM *(extension)* |
| VECTOR_PIPELINES | PIPELINE_CATALOG, PIPELINE_SCHEMA, PIPELINE_NAME, SOURCE_SCHEMA, SOURCE_NAME, TEXT_COLUMN, CHUNKING, MODEL_SCHEMA, MODEL_NAME, TARGET_SCHEMA, TARGET_NAME, MODE | vector pipeline of a schema the user owns: its source table and text column, its chunking, its model, its target table, and SYNC or ASYNC *(extension)* ([CREATE VECTOR PIPELINE](statements/create-vector-pipeline.md)) |
| EMBEDDING_QUEUE | TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, MODEL_VERSION, PENDING, EMBEDDED, FAILURES, LAST_ERROR | column the queue after commit fills, of a table of a schema the user owns, or of any to the database's owner: its model's version; the rows waiting as its last round began; those given a vector and the rounds failed since the database opened; the last round's error, or NULL *(extension)* ([CREATE VECTOR PIPELINE](statements/create-vector-pipeline.md)) |

VALUE_SOURCE says where a value comes from: DEFAULT, or the place it was
set: DATABASE, SESSION, or NODE for a node's setting, whose value is
this node's alone, kept nowhere ([Settings](../appendix/c-settings.md)).
[SET](statements/set-setting.md) and
[SET DATABASE](statements/set-database.md) set the settings;
[Settings](../appendix/c-settings.md) lists them.

```sql
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE, IS_SESSION_SETTABLE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME IN ('LOCKS.WAIT', 'MEMORY.BUDGET_BYTES', 'SESSION.TIME_ZONE');
SELECT * FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS;
```

```
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE | IS_SESSION_SETTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------+---------------------
 LOCKS.WAIT                                                   | OFF                                                          | DEFAULT      | YES
 MEMORY.BUDGET_BYTES                                          | 1073741824                                                   | DEFAULT      | NO
 SESSION.TIME_ZONE                                            | +00:00                                                       | DEFAULT      | YES
(3 rows)
 RESERVED_BY                                                  |                BYTES
--------------------------------------------------------------+----------------------
 the buffer pool of the data file                             |            134217728
(1 row)
```

The second, of a database kept in a directory: one in memory has no
buffer pool. MEMORY_RESERVATIONS, FILES, JOBS, CONNECTIONS and SERVER show their
rows to the database's owner alone. CONNECTIONS and SERVER are a
server's: a database a server serves, as `turboreld`'s, has them, and
they show no connection, and counts of none, while it does not serve.
JOBS.POLICY says how the jobs JOBS lists are run
([Settings](../appendix/c-settings.md)).

The standard: ISO/IEC 9075:1992 §21.1, §21.2; the views of roles and of
triggers, and how each view reads the enabled roles, the 2006 draft of
ISO/IEC 9075-11, Clauses 5 and 6.
