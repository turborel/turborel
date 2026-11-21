# CREATE TABLE

[Statements](README.md)

## NAME

CREATE TABLE: define a base table

## SYNOPSIS

```
<table definition> ::=
    CREATE [ { GLOBAL | LOCAL } TEMPORARY ] TABLE <table name> { <table contents source> | <partition of clause> }
    [ <graph table kind> ]
    [ <partition by clause> ]
    [ WITH SYSTEM VERSIONING ]
    [ ON COMMIT { DELETE | PRESERVE } ROWS ]
    [ <partition by clause> ]
    [ USING <engine name> ]
    [ WITH ( <table option> [ { , <table option> }... ] ) ]

<table contents source> ::=
      ( <table element> [ { , <table element> }... ] )
    | OF <user-defined type name> [ UNDER <supertable name> ] [ ( <typed table element> [ { , <typed table element> }... ] ) ]
    | [ ( <column name> [ { , <column name> }... ] ) ] AS ( <query expression> ) WITH [ NO ] DATA

<graph table kind> ::= AS { NODE | EDGE }

<partition by clause> ::= PARTITION BY <strategy> ( <key part> [ { , <key part> }... ] )

<key part> ::= <column name> | ( <value expression> )

<partition of clause> ::= PARTITION OF <table name> <partition bound>

<partition bound> ::= DEFAULT
    | FOR VALUES FROM ( <range datum> [ { , <range datum> }... ] ) TO ( <range datum> [ { , <range datum> }... ] )
    | FOR VALUES IN ( <list value> [ { , <list value> }... ] )
    | FOR VALUES WITH ( <option name> <literal> [ { , <option name> <literal> }... ] )

<table element> ::= <column definition> | <table constraint definition> | <like clause> | <table period definition>

<table period definition> ::= PERIOD FOR { <period name> | SYSTEM_TIME } ( <column name> , <column name> )

<typed table element> ::=
      REF IS <column name> { SYSTEM GENERATED | USER GENERATED | DERIVED }
    | <column name> WITH OPTIONS <column option>...
    | <table constraint definition>

<column option> ::= SCOPE <table name> | DEFAULT <default option> | <column constraint definition>

<like clause> ::= LIKE <table name> [ <like option>... ]

<like option> ::= { INCLUDING | EXCLUDING } { IDENTITY | DEFAULTS | GENERATED }

<column definition> ::=
    <column name> [ <data type> | <domain name> ]
    [ DEFAULT <default option> | <identity column specification> | <generation clause>
      | GENERATED ALWAYS AS ROW { START | END } ]
    [ <column constraint definition>... ]

<identity column specification> ::=
    GENERATED { ALWAYS | BY DEFAULT } AS IDENTITY
    [ ( <common sequence generator option>... ) ]

<common sequence generator option> ::=
      START WITH <signed numeric literal>
    | INCREMENT BY <signed numeric literal>
    | MAXVALUE <signed numeric literal> | NO MAXVALUE
    | MINVALUE <signed numeric literal> | NO MINVALUE
    | CYCLE | NO CYCLE

<generation clause> ::= GENERATED ALWAYS AS ( [ WITH <XML lexically scoped options> ] <value expression> ) [ STORED ] [ SYNC | ASYNC ]

<default option> ::=
      <literal>
    | USER | CURRENT_USER | CURRENT_ROLE | SESSION_USER | SYSTEM_USER
    | CURRENT_DATE | CURRENT_TIME [ ( <precision> ) ] | CURRENT_TIMESTAMP [ ( <precision> ) ]
    | NULL

<column constraint definition> ::=
    [ CONSTRAINT <constraint name> ]
    { NOT NULL | UNIQUE | PRIMARY KEY | <references specification> | <check constraint definition> }
    [ <constraint attributes> ]

<table constraint definition> ::=
    [ CONSTRAINT <constraint name> ]
    {   { UNIQUE [ NULLS [ NOT ] DISTINCT ] | PRIMARY KEY }
          ( <column name> [ { , <column name> }... ] [ , <period name> WITHOUT OVERLAPS ] )
      | FOREIGN KEY ( <column name> [ { , <column name> }... ] [ , PERIOD <period name> ] )
          <references specification>
      | <check constraint definition>
      | <connection constraint> }
    [ <constraint attributes> ]

<connection constraint> ::=
    CONNECTION ( <table name> TO <table name> [ { , <table name> TO <table name> }... ] )
    [ ON DELETE { CASCADE | NO ACTION } ]

<references specification> ::=
    REFERENCES <table name> [ ( <column name> [ { , <column name> }... ] [ , PERIOD <period name> ] ) ]
    [ MATCH { FULL | PARTIAL } ]
    [ ON UPDATE <referential action> ] [ ON DELETE <referential action> ]

<referential action> ::= CASCADE | SET NULL | SET DEFAULT | RESTRICT | NO ACTION

<check constraint definition> ::= CHECK ( [ WITH <XML lexically scoped options> ] <search condition> )

<constraint attributes> ::=
      INITIALLY { DEFERRED | IMMEDIATE } [ [ NOT ] DEFERRABLE ]
    | [ NOT ] DEFERRABLE [ INITIALLY { DEFERRED | IMMEDIATE } ]

<table option> ::= <option name> = { <option value> | DEFAULT }
```

A column has a data type or a domain unless it has a generation clause.
A node or edge table may write no `<table contents source>` before AS
NODE or AS EDGE, and then has only its pseudo-columns. NODE, EDGE and CONNECTION's
TO are read in their places alone, so NODE and EDGE may name a table or
column.
GENERATED, ALWAYS and EXPRESSION are non-reserved words, so a domain may
be named GENERATED. INCLUDING, EXCLUDING, DEFAULTS, DATA and NULLS are
non-reserved words too. ON UPDATE and ON DELETE may be written in either order, each once. The
data types are in [Data Types](../02-data-types.md); a column may also
be of a type a plug-in registers *(extension)*, written by its name and,
in parentheses, one or two unsigned integers it takes.

PARTITION BY and PARTITION OF are extensions, a `<partition by clause>`
written once *(extension)*. PARTITION is read there only before BY or
OF, and MINVALUE and MAXVALUE only within a bound.

## DESCRIPTION

PARTITION BY makes a partitioned table, which holds no rows itself. Its
rows are kept in its partitions, each in the one whose bound holds its
key, by the strategy PARTITION BY names. PARTITION OF makes a partition of a
partitioned table, of its columns, defaults, CHECK constraints and
periods, and may partition it again. Both are described in
[Partitioned Tables](../23-partitioning.md) *(extension)*.

CREATE TABLE defines a base table: its columns, each of a data type or a
domain, its constraints, and, *(extension)*, the storage engine that
keeps its rows and that engine's options.

A column's default is its own, written DEFAULT, else its domain's, else
the null value. A datetime literal written without a time zone takes
the session's ([SET TIME ZONE](set-time-zone.md)); CURRENT_DATE and the
others are taken as the statement that inserts the row runs. A binary
string shorter than its BINARY column is padded with X'00' octets.

An identity column takes its values from a sequence generator of its
own, made with the table and dropped with the column. Its options are
those of [CREATE SEQUENCE](create-sequence.md), and its type, an exact
numeric type of scale 0, is the generator's. A row inserted with no
value for it, or with DEFAULT, takes the generator's next value, as does
a row an UPDATE sets to DEFAULT. A value of GENERATED ALWAYS is given
only by the generator, unless an INSERT writes OVERRIDING SYSTEM VALUE.
GENERATED BY DEFAULT takes a value given it too. A value once drawn is
never drawn again, even if the transaction that drew it rolls back.
An identity column is NOT NULL NOT DEFERRABLE, a constraint the table is
given if none is written.

A CHECK's and a generation expression's XML options, XMLNAMESPACES and
XMLBINARY, are in scope of it, and kept with its text: see
[XML Values](../19-xml.md#xml-options-of-a-scope). The Information Schema's CHECK_CLAUSE
shows the search condition without them.

A generated column holds the value of its expression, computed from the
other columns of its row as the row is inserted or updated and kept with
the row. Its type is the one written, to which the expression's value is
assigned, else the expression's own. A statement gives it no value of its
own. An INSERT gives it only DEFAULT, and an UPDATE sets it only to
DEFAULT. Constraints and indexes take its values as any column's.

A generated column is STORED, whether or not that is written. SYNC, the
default, computes it as the row is written. ASYNC, of a column whose
expression embeds a text (see [CREATE EMBEDDING
MODEL](create-embedding-model.md)), leaves it null as the row is
inserted, or updated in a column the expression reads, by INSERT,
UPDATE or MERGE, and the queue after commit fills it later. An update
that sets it to DEFAULT computes it then.

**Constraints.** NOT NULL refuses the null value. UNIQUE refuses two
rows of the same values in its columns, of rows none of whose values is
null. UNIQUE NULLS NOT DISTINCT takes a null as a value too, the same as
another null, so it refuses a second row of a null where a row has one
and the same values in the other columns, and two rows all null. One
column of it holds one null at most. NULLS DISTINCT is the default.
PRIMARY KEY is UNIQUE and NOT NULL, one to a table. WITHOUT OVERLAPS
of the table's period, after the columns, makes two rows of the same
values a duplicate only if their periods overlap. So a key holds one
row at each point in time, and a row may end where the next of its key
begins. A foreign key
refuses a row whose values do not match a row of the referenced table:
of its PRIMARY KEY, or of the columns named, which a UNIQUE or PRIMARY
KEY constraint is on, not one WITHOUT OVERLAPS. Its MATCH says how a row with nulls in the foreign
key's columns is taken: with none written, it is accepted if any of
them is null; MATCH FULL accepts it only if all are null, and refuses
one with some null and some not; MATCH PARTIAL accepts it if its values
that are not null match a row's. ON UPDATE and ON DELETE say what is done to the
rows that refer to one changed or deleted: refused (NO ACTION, the
default), changed or deleted with it (CASCADE), or their values set null
or to their defaults. NO ACTION judges the rows as the statement ends,
or at commit if deferred. RESTRICT refuses at once a row deleted, or a
referenced value changed, while a row referenced it before the
statement (SQLSTATE 23001), so a statement that deletes a row and the
rows that refer to it is refused, deferred or not.

A foreign key of a period, PERIOD after its columns and after the
columns it references, keeps each row's period within the periods of
the rows its values match. One row may span several that meet, as a
price held across two of its periods. The columns it references are a
key WITHOUT OVERLAPS of the referenced period. A row of the referenced
table deleted, or its period shortened, is refused while a row that
refers to it is then left outside them. A CHECK refuses a row for which its condition is
false; it may read other tables through subqueries.

**System time.** PERIOD FOR SYSTEM_TIME names a column GENERATED ALWAYS
AS ROW START and one GENERATED ALWAYS AS ROW END as the table's
system-time period, which the database gives every row. A row written
begins at the transaction timestamp, the instant the transaction first
changed data, and ends at the highest value of its type, 9999-12-31 of
DATE. WITH SYSTEM VERSIONING makes the table system-versioned: a row a
later transaction updates or deletes is kept as a historical row, which
ends at that transaction's timestamp. Statements read, key and
constrain the current rows alone, unless a query reads the table FOR
SYSTEM_TIME ([Query Expressions](../06-queries.md#reading-a-tables-history)).
A transaction that changes a row
another began after its own timestamp is refused, SQLSTATE 2201H. A
system-versioned table may have an application-time period too, a
bitemporal table: UPDATE and DELETE FOR PORTION OF it keep each row
they change as a historical row, the parts they keep beginning at the
transaction timestamp, and its keys judge the current rows alone.

A constraint is checked at the end of each statement, or, DEFERRABLE
INITIALLY DEFERRED or deferred by [SET CONSTRAINTS](set-constraints.md),
at commit. A constraint not named is given a name in the table's
schema: the table's, its kind, and a number, as `EMPLOYEES_PRIMARY1`. A
constraint's name written without a schema is of the session's default
schema, which may be another than the table's: in a session whose
default schema is ANN, `CREATE TABLE SHOP.ITEMS (ID INTEGER CONSTRAINT
KEYED PRIMARY KEY)` names its constraint ANN.KEYED. In a [CREATE
SCHEMA](create-schema.md), it is of the schema being defined.

**Periods.** PERIOD FOR names two columns of the table as a period,
from its start column, included, to its end column, excluded. The table
gets the period constraint CHECK (start < end), named for the table and
the period with `_PERIOD` after, as `ROOMS_STAY_PERIOD`, and numbered if
its schema has that name. While the period is, its columns are not
dropped, nor made nullable. [ALTER TABLE](alter-table.md) adds or drops
one. The view PERIODS of the [Information Schema](../07-information-schema.md)
shows each.

**Temporary tables.** GLOBAL TEMPORARY or LOCAL TEMPORARY defines a
table whose rows are each session's own: empty as the session begins,
gone when it ends, and never recovered. ON COMMIT DELETE ROWS, the
default, empties them as each transaction ends; PRESERVE ROWS keeps
them. See [Temporary Tables](../../user/09-temporary-tables.md).

**Storage** *(extension)*. USING names the engine that keeps the table's
rows, else the database's setting TABLES.ENGINE; WITH sets the engine's
options for the table. INFORMATION_SCHEMA.TABLE_OPTIONS shows them, and
[Appendix G](../../appendix/g-table-options.md) lists the built-in
engines'. A temporary table names neither.

**Row store or column store** *(extension)*. The built-in engines are two:
`rowstore`, the default, which keeps each row's values together, and
`columnstore`, which keeps each column's values together once rows are
frozen into groups. A table that is read in bulk, a few of its columns at
a time, reads faster in the column store; one whose rows are mostly
fetched by key, and changed, in the row store. Its rows, and what queries
of it give, are the same in either; see [Appendix
G](../../appendix/g-table-options.md).

LIKE copies the columns of another table, or of a view, in its place
among the table's other elements. Each column copied has the name and
the data type of the one it copies, its collation with it, and NOT NULL
if that column can hold no null value. Nothing else is copied unless an
option says so. INCLUDING IDENTITY makes the identity column one again,
with the same options, as a sequence generator of the new table's own
that starts at the start value. INCLUDING DEFAULTS copies each column's
default, and INCLUDING GENERATED each generation expression, the other
table's name in it replaced by the new table's. EXCLUDING, the default,
copies none. No constraint but NOT NULL is copied, no domain and no row.

AS defines the table from a query. Each column of the query becomes a
column of the table, named as the query names it or as the column names
written before AS do, of the query column's type and collation, and NOT
NULL if the query column can hold no null value. WITH DATA inserts the
query's rows, in the same statement, and WITH NO DATA leaves the table
empty. The query may draw values of a sequence generator with NEXT VALUE
FOR. If inserting the rows fails, no table is defined.

The table named by LIKE, and the tables and views the query reads, are
held shared until the transaction ends, so no other transaction alters
or drops one meanwhile. The table keeps no tie to them after that.

A typed table, `OF` a structured type, has a column of each attribute
of the type, in order, and first a self-referencing column, named by REF
IS, whose value identifies its row. The type's reference type
specification says how that value is made: SYSTEM GENERATED by turborel,
never given twice, USER GENERATED by the INSERT, or DERIVED from
attributes a unique constraint keeps. A typed table `UNDER` another is
its subtable, of a direct subtype of the supertable's type. A row of a
subtable is a row of each supertable too, so a supertable read without
ONLY reads its subtables' rows, cut to its columns, and its constraints
hold of them. WITH OPTIONS gives a column a SCOPE, a default or
constraints. ALTER TABLE adds or drops no column of a typed table.

AS NODE or AS EDGE *(extension)* makes a node table or an edge table,
the native graph tables of the property graph standard's Mode B. A node
table is given a hidden column `$NODE_ID`, and an edge table `$EDGE_ID`,
`$FROM_ID` and `$TO_ID`, after its own columns. These pseudo-columns are
VARCHAR(2048) and NOT NULL. `*` does not show them, nor does an INSERT
with no column list fill them, but a query names them like any other
column, in any case. COLUMNS in the [Information
Schema](../07-information-schema.md) shows each with IS_HIDDEN YES.

Each row of a node table is given an identity in `$NODE_ID` as it is
inserted, and each edge one in `$EDGE_ID`. The identity is the text
`{"type":"node","schema":"S","table":"T","id":N}`, N counting from 0 in
each table. No INSERT or UPDATE writes it. A number once given is not
given again, but numbers may be skipped after the database reopens.

An edge is inserted with the identities of the two nodes it joins, in
`$FROM_ID` and `$TO_ID`, written as text or read by a subquery. Each is
the identity of a node of a node table of the edge table's schema, kept
in the one form above, so the same node written with its members in
another order or spacing is still one node. Without a CONNECTION
constraint, an edge may join any two nodes of the schema. A CONNECTION
constraint lists the pairs of node tables, FROM TO, its edges may join,
and each edge's endpoints must then be rows of those tables, as INSERT
and UPDATE check at the end of the statement. With several CONNECTION
constraints, an edge satisfies each.

A node is not deleted while an edge of its schema joins it, unless
every edge table that has such an edge has a CONNECTION constraint ON
DELETE CASCADE that names the node's table. Those edges are then
deleted with it, in the same statement. TRUNCATE of a node table is
refused while its schema has an edge table.

Each edge table has an adjacency index of its endpoints, named
`S.T.ADJACENCY`, and each node and edge table a B-tree of its identity,
named `S.T.$NODE_ID` or `S.T.$EDGE_ID`. INDEXES shows them. The node and
edge tables of a schema make up its property graph SYS_GRAPH, which the
database keeps. Each node table is a vertex table of it, keyed by
`$NODE_ID` and labelled by its name, with its own columns as
properties. Each edge table is an edge table of it once for each pair of
node tables it may join, keyed by `$EDGE_ID` and labelled by its name.
When it joins one pair, its alias is its name; else it is the edge
table's name and the pair's, as in `KNOWS PERSON TO CITY`. SYS_GRAPH
changes as the tables are created, altered and dropped, and goes with
the schema's last node table. No statement creates, alters or drops it.
GRAPH_TABLES and CONNECTION_CONSTRAINTS in the Information Schema show
the node and edge tables and the clauses of each CONNECTION constraint.

ALTER TABLE adds and drops a node or edge table's own columns, and adds
or drops a CONNECTION constraint, which its edges must satisfy as it is
added. It drops and alters no pseudo-column, and adds no system
versioning. DROP TABLE of a node table that a CONNECTION constraint
names needs CASCADE, which drops the constraint.

CREATE TABLE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The table's schema exists, and its user owns it. A table named without
   a schema is of the session's default schema.
2. No table or view of the schema has the name; no column is defined
   twice; the table has a column.
3. A constraint's name is new in its schema, which exists, and which
   the table's schema's owner owns *(extension)*; in a CREATE SCHEMA, its
   schema is the one being defined; it is not of MODULE but for a
   declared local temporary table, whose constraints' are.
4. A table has at most one PRIMARY KEY, and no two unique constraints on
   the same columns.
5. A foreign key's referenced table is a base table, its referenced
   columns those of a unique constraint, as many as the referencing
   ones, each of the same type.
6. A CHECK contains no USER, CURRENT_USER, CURRENT_ROLE, SESSION_USER or
   SYSTEM_USER,
   no CURRENT_DATE, CURRENT_TIME or CURRENT_TIMESTAMP, and is not
   possibly non-deterministic. A call of a function the program
   registered is possibly non-deterministic if the function says it is
   not deterministic *(extension)*.
7. A default is of the column's type: a literal it can hold, a user
   function of a character column of at least 128 characters, or a
   datetime value function of its fields.
8. ON COMMIT is of a temporary table; USING and WITH are not.
9. Of temporary tables, which may refer to which follows SQL-92: a
   persistent table to persistent ones; a global temporary to global
   ones; one that keeps its rows at commit to none that deletes them.
10. The engine is registered, and takes each option written, of a value
    it accepts.
11. The transaction is not READ ONLY.
12. A table has at most one identity column, of an exact numeric type of
    scale 0, and its options are as CREATE SEQUENCE's rules say of that
    type.
13. A generation expression references only columns of its table that
    are not generated columns. It contains no subquery, no set function,
    no USER or other user function, no CURRENT_DATE or other datetime
    value function, and is not possibly non-deterministic, a registered
    function's call included, as in a CHECK. Its type is assignable to
    the column's.
14. A foreign key whose referencing columns include a generated column
    has no SET NULL or SET DEFAULT action and no ON UPDATE CASCADE.
15. LIKE names a table or view that exists, and writes each of its
    options once. Of a view, it writes none. It names no declared local
    temporary table, but in [DECLARE LOCAL TEMPORARY
    TABLE](declare-local-temporary-table.md).
16. Of AS, each column of the query has a name, and no two have the same
    one, unless column names are written: as many as the query's
    columns, no two the same. The query has no dynamic parameter and
    reads no declared local temporary table. AS is not written in a
    [CREATE SCHEMA](create-schema.md).
17. NULLS DISTINCT or NULLS NOT DISTINCT follows UNIQUE in a table
    constraint, not PRIMARY KEY, and not in a column constraint.
18. A table has one period at most, and a temporary table none. No
    column is named as the period. Its columns are two of the table's,
    each written as DATE, or both as one timestamp type, each NOT NULL
    and not DEFERRABLE, or of the PRIMARY KEY, and neither an identity
    nor a generated column. A table has one PERIOD FOR SYSTEM_TIME at
    most beside it.
19. WITHOUT OVERLAPS names the table's period, after one column at
    least. No two UNIQUE or PRIMARY KEY constraints are of the same
    columns, whatever their period. A foreign key with no columns
    written references no PRIMARY KEY WITHOUT OVERLAPS.
20. A foreign key writes PERIOD after its columns and after those it
    references, or neither, the columns it references written. They are
    those of a key WITHOUT OVERLAPS of that period of the table
    referenced, and its own period is of the table, of the same type.
    It has no action CASCADE, SET NULL or SET DEFAULT.
21. A column of ROW START is the start of the table's PERIOD FOR
    SYSTEM_TIME and one of ROW END its end, each DATE or a timestamp
    type, NOT NULL implicitly, of no default. WITH SYSTEM VERSIONING
    needs the period, of a table with no column of a reference type,
    and no LIKE names a system-versioned table. No CHECK, UNIQUE,
    PRIMARY KEY or foreign key is of a ROW START or ROW END column.
22. AS NODE and AS EDGE *(extension)* make no temporary, typed or
    system-versioned table, and none from a query. The schema has no
    table or view named SYS_GRAPH. A CONNECTION constraint is of an
    edge table, and names node tables of its schema (GRF-0004).

## PRIVILEGES

The table's schema is its user's. REFERENCES on each column a foreign
key references, and, for a CHECK, on each column it reads of another
table, or, if it names none, on one column of each table it reads.
USAGE on each domain a column is of. SELECT on the table or view LIKE
names. The privileges the query of AS needs to be run.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist, or a constraint's does not |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or a constraint's, or lacks REFERENCES or USAGE it needs |
| 25000 | -917 | The transaction is READ ONLY |
| 0A000 | -900 | A CHARACTER SET clause of another character set |
| 2A000 | -103 | A COLLATE clause of no collation, or of a column not of a character string |
| 02000 | 100 | Not an error: the query of AS ... WITH DATA had no row |
| 2A000 | -103 | A CONNECTION constraint names no node table of the schema, GRF-0004 *(extension)* |
| 23000 | -201 | An edge's endpoint is no node, or of tables no CONNECTION constraint allows, GRF-0053 *(extension)* |
| 23000 | -201 | A node deleted has edges no CONNECTION constraint deletes with it, GRF-0052 *(extension)* |
| 2A000 | -103 | A partition bound overlaps another's, or is a second DEFAULT, a key part is not there or is collated, the strategy is not registered, or the table is typed or a node or edge table *(extension)* |

Any error running the query of AS ... WITH DATA, or inserting its rows,
is the statement's.

## EXAMPLES

```sql
CREATE TABLE DEPARTMENTS (CODE CHAR(3) PRIMARY KEY, NAME VARCHAR(30) NOT NULL UNIQUE);
CREATE TABLE EMPLOYEES (
    ID INTEGER NOT NULL,
    NAME VARCHAR(30) NOT NULL,
    DEPT CHAR(3) REFERENCES DEPARTMENTS ON DELETE SET NULL,
    HIRED DATE DEFAULT CURRENT_DATE,
    SALARY DECIMAL(9,2) CONSTRAINT POSITIVE_PAY CHECK (SALARY > 0),
    PRIMARY KEY (ID));
COMMIT;
SELECT CONSTRAINT_NAME, CONSTRAINT_TYPE FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS
    WHERE TABLE_NAME = 'EMPLOYEES' ORDER BY 1;
```

```
OK
OK
OK
 CONSTRAINT_NAME                                              | CONSTRAINT_TYPE
--------------------------------------------------------------+--------------------------------------------------------------
 EMPLOYEES_FOREIGN1                                           | FOREIGN KEY
 EMPLOYEES_NOTNULL1                                           | CHECK
 EMPLOYEES_NOTNULL2                                           | CHECK
 EMPLOYEES_PRIMARY1                                           | PRIMARY KEY
 POSITIVE_PAY                                                 | CHECK
(5 rows)
```

NOT NULL is a CHECK constraint, as the Information Schema shows it.

Of the column store, its rows frozen a minute after they are committed
at the soonest, each group of them sorted by region *(extension)*:

```sql
CREATE TABLE SALES (ID INTEGER PRIMARY KEY, REGION CHAR(8), AMOUNT DECIMAL(9,2))
    USING columnstore WITH (COLUMNSTORE.FREEZE_DELAY = 60, COLUMNSTORE.ORDER = 'region');
COMMIT;
SELECT ENGINE_NAME, OPTION_NAME, OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS
    WHERE TABLE_NAME = 'SALES' AND VALUE_SOURCE = 'TABLE' ORDER BY 2;
```

```
OK
OK
 ENGINE_NAME                                                  | OPTION_NAME                                                  | OPTION_VALUE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 columnstore                                                  | COLUMNSTORE.FREEZE_DELAY                                     | 60
 columnstore                                                  | COLUMNSTORE.ORDER                                            | REGION
(2 rows)
```

The option names the column as the table does: `region`, a regular
identifier, is REGION.

An identity column and a generated column:

```sql
CREATE TABLE ORDERS (
    ID INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 100),
    QUANTITY INTEGER,
    PRICE DECIMAL(9,2),
    TOTAL GENERATED ALWAYS AS (QUANTITY * PRICE));
INSERT INTO ORDERS (QUANTITY, PRICE) VALUES (2, 4.50), (10, 1.25);
SELECT * FROM ORDERS;
```

```
OK
OK
           ID |     QUANTITY |       PRICE |                 TOTAL
--------------+--------------+-------------+-----------------------
          100 |            2 |        4.50 |                  9.00
          101 |           10 |        1.25 |                 12.50
(2 rows)
```

A registered function that is not deterministic, as EMBED_VERSION,
whose answer changes as its model does, is refused in a CHECK and in a
generation expression *(extension)*:

```sql
CREATE TABLE TAGS (MODEL VARCHAR(100) CHECK (EMBED_VERSION(MODEL) IS NOT NULL));
CREATE TABLE TAGS (MODEL VARCHAR(100), V VARCHAR(100) GENERATED ALWAYS AS (EMBED_VERSION(MODEL)));
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): CHECK ANN.TAGS_CHECK1: a CHECK condition shall not contain a potential source of non-determinism: a call of function EMBED_VERSION, registered, which is not deterministic [ISO/IEC 9075-2:2023 §9.16 SR1am, §11.9 CR2] (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): the generation expression of column V of ANN.TAGS: a generation expression shall not contain a call of function EMBED_VERSION, registered, which is not deterministic [ISO/IEC 9075-2:2023 §11.4 SR10d, §9.16 SR1am] (at character 39)
```

A table that copies another's columns, its identity column and its
defaults, and one defined from a query:

```sql
CREATE TABLE TICKETS (
    ID INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 10),
    NAME VARCHAR(20) DEFAULT 'none' NOT NULL,
    PRICE DECIMAL(7,2) CHECK (PRICE > 0));
INSERT INTO TICKETS (NAME, PRICE) VALUES ('first', 5), ('second', 7.50);
CREATE TABLE ARCHIVE (LIKE TICKETS INCLUDING IDENTITY INCLUDING DEFAULTS, MOVED DATE);
INSERT INTO ARCHIVE (PRICE) VALUES (-1);
SELECT * FROM ARCHIVE;
CREATE TABLE DEAR (NAME, DOUBLED) AS (SELECT NAME, PRICE * 2 FROM TICKETS WHERE PRICE > 6) WITH DATA;
SELECT * FROM DEAR;
CREATE TABLE NOT_YET AS (SELECT NAME FROM TICKETS WHERE PRICE > 100) WITH DATA;
```

```
OK
OK
OK
OK
           ID | NAME                 |     PRICE | MOVED
--------------+----------------------+-----------+------------
           10 | none                 |     -1.00 | NULL
(1 row)
OK
 NAME                 |    DOUBLED
----------------------+------------
 second               |      15.00
(1 row)
no data
```

The CHECK of TICKETS is not copied, so ARCHIVE takes a price of -1, and
its generator starts at 10, as that of TICKETS did. No row of TICKETS
costs more than 100, so the last table is defined with no data.

A key that lets one member of staff have no desk, not two:

```sql
CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, DESK INTEGER, CONSTRAINT ONE_UNSEATED UNIQUE NULLS NOT DISTINCT (DESK));
INSERT INTO STAFF VALUES (1, 10), (2, NULL);
INSERT INTO STAFF VALUES (3, NULL);
COMMIT;
SELECT CONSTRAINT_NAME, NULLS_DISTINCT FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS
    WHERE TABLE_NAME = 'STAFF' AND CONSTRAINT_TYPE = 'UNIQUE';
```

```
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): UNIQUE ANN.ONE_UNSEATED (DESK) of ANN.STAFF would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
OK
 CONSTRAINT_NAME                                              | NULLS_DISTINCT
--------------------------------------------------------------+--------------------------------------------------------------
 ONE_UNSEATED                                                 | NO
(1 row)
```

A period of the days a room is booked, from its first, included, to its
last, excluded:

```sql
CREATE TABLE ROOMS (ROOM INTEGER, FROM_DAY DATE NOT NULL, TO_DAY DATE NOT NULL, PERIOD FOR STAY (FROM_DAY, TO_DAY));
INSERT INTO ROOMS VALUES (101, DATE '2026-03-01', DATE '2026-03-04');
INSERT INTO ROOMS VALUES (102, DATE '2026-03-04', DATE '2026-03-04');
COMMIT;
SELECT PERIOD_NAME, CONSTRAINT_NAME FROM INFORMATION_SCHEMA.PERIODS;
```

```
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.ROOMS_STAY_PERIOD of ANN.ROOMS is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
OK
 PERIOD_NAME                                                  | CONSTRAINT_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 STAY                                                         | ROOMS_STAY_PERIOD
(1 row)
```

A key of the room booked, one booking of a room on each day. A booking
from the day another ends is kept, one that overlaps it is refused:

```sql
CREATE TABLE BOOKINGS (ROOM INTEGER, FROM_DAY DATE NOT NULL, TO_DAY DATE NOT NULL, PERIOD FOR STAY (FROM_DAY, TO_DAY), PRIMARY KEY (ROOM, STAY WITHOUT OVERLAPS));
INSERT INTO BOOKINGS VALUES (101, DATE '2026-03-01', DATE '2026-03-04');
INSERT INTO BOOKINGS VALUES (101, DATE '2026-03-04', DATE '2026-03-06');
INSERT INTO BOOKINGS VALUES (101, DATE '2026-03-05', DATE '2026-03-08');
COMMIT;
```

```
OK
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): PRIMARY KEY ANN.BOOKINGS_PRIMARY1 (ROOM, STAY WITHOUT OVERLAPS) of ANN.BOOKINGS would be violated: a row of these values is there for a time in its period [ISO/IEC 9075-2:2023 §11.7 SR6d i]
OK
```

Orders whose days the room's bookings cover. A stay across two bookings
is kept, one past them refused, and so is a booking cut short under one:

```sql
CREATE TABLE STAYS (ROOM INTEGER, IN_DAY DATE NOT NULL, OUT_DAY DATE NOT NULL, PERIOD FOR VISIT (IN_DAY, OUT_DAY), FOREIGN KEY (ROOM, PERIOD VISIT) REFERENCES BOOKINGS (ROOM, PERIOD STAY));
INSERT INTO STAYS VALUES (101, DATE '2026-03-02', DATE '2026-03-05');
INSERT INTO STAYS VALUES (101, DATE '2026-03-05', DATE '2026-03-07');
UPDATE BOOKINGS SET TO_DAY = DATE '2026-03-03' WHERE FROM_DAY = DATE '2026-03-01';
COMMIT;
```

```
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): a row of ANN.STAYS references no row of ANN.BOOKINGS: FOREIGN KEY ANN.STAYS_FOREIGN1 [ISO/IEC 9075:1992 §11.8 GR2]
turborel: SQLCODE -201 (SQLSTATE 23000): a row of ANN.STAYS references a row of ANN.BOOKINGS no longer there: FOREIGN KEY ANN.STAYS_FOREIGN1 [ISO/IEC 9075:1992 §11.8 GR2]
OK
```

A system-versioned table of prices. The update keeps the old price as a
historical row, and the current one ends at the highest timestamp. No
statement gives its period's columns a value, and its columns are kept:

```sql
CREATE TABLE PRICES (
    ITEM INTEGER PRIMARY KEY,
    PRICE DECIMAL(9,2),
    VALID_FROM TIMESTAMP(6) GENERATED ALWAYS AS ROW START,
    VALID_TO TIMESTAMP(6) GENERATED ALWAYS AS ROW END,
    PERIOD FOR SYSTEM_TIME (VALID_FROM, VALID_TO))
    WITH SYSTEM VERSIONING;
INSERT INTO PRICES (ITEM, PRICE) VALUES (1, 9.99);
COMMIT;
UPDATE PRICES SET PRICE = 12.50;
COMMIT;
SELECT ITEM, PRICE, VALID_TO FROM PRICES;
UPDATE PRICES SET VALID_TO = CURRENT_TIMESTAMP;
INSERT INTO PRICES VALUES (2, 1.00, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
ALTER TABLE PRICES ADD COLUMN NOTE VARCHAR(20);
SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'PRICES';
```

```
OK
OK
OK
OK
OK
         ITEM |       PRICE | VALID_TO
--------------+-------------+----------------------------
            1 |       12.50 | 9999-12-31 23:59:59.999999
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): column VALID_TO is a system-time period column, so no <set clause> sets it [ISO/IEC 9075-2:2023 §14.15 SR3] (at character 18)
turborel: SQLCODE -103 (SQLSTATE 2A000): column VALID_FROM is a system-time period column, so a value is given it only with OVERRIDING USER VALUE [ISO/IEC 9075-2:2023 §14.11 SR13d] (at character 36)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.PRICES is a system-versioned table, whose columns are not added, altered or dropped [ISO/IEC 9075-2:2023 §11.11 SR2] (at character 30)
 TABLE_NAME                                                   | TABLE_TYPE
--------------------------------------------------------------+--------------------------------------------------------------
 PRICES                                                       | SYSTEM VERSIONED
(1 row)
```

Tables refused:

```sql
CREATE TABLE T3 (A INTEGER PRIMARY KEY, B INTEGER PRIMARY KEY);
CREATE TABLE T5 (A INTEGER REFERENCES EMPLOYEES (NAME));
CREATE TABLE T9 (A INTEGER DEFAULT 'x');
CREATE TABLE T7 (A INTEGER) WITH (COLUMNSTORE.GROUP_ROWS = 4096);
CREATE TABLE T8 (A INTEGER) USING columnstore WITH (COLUMNSTORE.ORDER = 'B');
CREATE TABLE T10 (A REAL GENERATED ALWAYS AS IDENTITY);
CREATE TABLE T11 (A INTEGER, B GENERATED ALWAYS AS (A + (SELECT COUNT(*) FROM ORDERS)));
CREATE TABLE T12 (LIKE ORDERS INCLUDING DEFAULTS EXCLUDING DEFAULTS);
CREATE TABLE T13 AS (SELECT QUANTITY * PRICE FROM ORDERS) WITH NO DATA;
CREATE TABLE T14 (A INTEGER, PRIMARY KEY NULLS NOT DISTINCT (A));
CREATE TABLE T15 (A DATE NOT NULL, B TIMESTAMP NOT NULL, PERIOD FOR P (A, B));
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.T3 has more than one PRIMARY KEY [ISO/IEC 9075:1992 §11.7 SR5, SR6] (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): the columns referenced are not those of a unique constraint of ANN.EMPLOYEES [ISO/IEC 9075:1992 §11.8 SR2a] (at character 27)
turborel: SQLCODE -103 (SQLSTATE 2A000): the default is not a <literal> of INTEGER [ISO/IEC 9075:1992 §11.5 SR3a] (at character 17)
turborel: SQLCODE -103 (SQLSTATE 2A000): rowstore takes no option COLUMNSTORE.GROUP_ROWS (at character 34)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.T8 has no column B, for the table option COLUMNSTORE.ORDER (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): an identity column is of an exact numeric type of scale 0, not REAL [ISO/IEC 9075-2:2023 §11.4 SR16a] (at character 25)
turborel: SQLCODE -103 (SQLSTATE 2A000): the generation expression of column B of ANN.T11: it contains no <query expression> [ISO/IEC 9075-2:2023 §11.4 SR10f] (at character 29)
turborel: SQLCODE -103 (SQLSTATE 2A000): a <like clause> writes its <column default option> once [ISO/IEC 9075-2:2023 §11.3 SR9b iv 1] (at character 49)
turborel: SQLCODE -103 (SQLSTATE 2A000): column 1 of the query has no name, so a <column name list> names it [ISO/IEC 9075-2:2023 §11.3 SR10b] (at character 17)
turborel: SQLCODE -103 (SQLSTATE 2A000): PRIMARY KEY writes no <unique null treatment>, as its columns are not null [ISO/IEC 9075-2:2023 §11.7 SR5] (at character 41)
turborel: SQLCODE -103 (SQLSTATE 2A000): the columns of period P are of one type, DATE or a timestamp type, not DATE and TIMESTAMP(6) [ISO/IEC 9075-2:2023 §11.3 SR2e iii] (at character 71)
```

The node and edge tables of a small social graph. The edge `LIVES`
joins people to towns alone, and goes with either.

```sql
CREATE TABLE PEOPLE (NAME VARCHAR(10)) AS NODE;
CREATE TABLE TOWNS (TOWN VARCHAR(10)) AS NODE;
CREATE TABLE LIVES (SINCE INTEGER, CONSTRAINT LIVES_LINK CONNECTION (PEOPLE TO TOWNS) ON DELETE CASCADE) AS EDGE;
INSERT INTO PEOPLE VALUES ('Ann');
INSERT INTO TOWNS VALUES ('Rome');
INSERT INTO LIVES ($FROM_ID, $TO_ID, SINCE) SELECT P.$NODE_ID, T.$NODE_ID, 2020 FROM PEOPLE P, TOWNS T;
SELECT * FROM PEOPLE;
SELECT $NODE_ID, NAME FROM PEOPLE;
SELECT P.NAME, T.TOWN, L.SINCE FROM PEOPLE P, LIVES L, TOWNS T WHERE L.$FROM_ID = P.$NODE_ID AND L.$TO_ID = T.$NODE_ID;
INSERT INTO LIVES ($FROM_ID, $TO_ID) SELECT T.$NODE_ID, P.$NODE_ID FROM PEOPLE P, TOWNS T;
DELETE FROM TOWNS;
SELECT COUNT(*) FROM LIVES;
```

```
OK
OK
OK
OK
OK
OK
 NAME
------------
 Ann
(1 row)
 $NODE_ID                                                     | NAME
--------------------------------------------------------------+------------
 {"type":"node","schema":"ANN","table":"PEOPLE","id":0}       | Ann
(1 row)
 NAME       | TOWN       |        SINCE
------------+------------+--------------
 Ann        | Rome       |         2020
(1 row)
turborel: SQLCODE -201 (SQLSTATE 23000): GRF-0053 an edge of ANN.LIVES joins a node of ANN.TOWNS to one of ANN.PEOPLE, which CONNECTION ANN.LIVES_LINK does not allow
OK

----------------------
                    0
(1 row)
```

## STANDARD

ISO/IEC 9075:1992 §11.3 to §11.9, and §10.6 for constraint names and
attributes; X3.135-1986 §6.2 to §6.4, whose tables have NOT NULL and
UNIQUE only. Identity columns and generated columns are ISO/IEC
9075-2:2023 §4.17.12, §4.17.13 and §11.4, Features T174 and T175. LIKE
is ISO/IEC 9075-2:2023 §11.3, Feature T171, and its options Feature
T173; AS is Feature T172. Periods are ISO/IEC 9075-2:2023 §4.16 and
§11.3, Feature T181, the name of the period constraint left to the
implementation, and so are WITHOUT OVERLAPS, §11.7, and a foreign key
of a period, §11.8; RESTRICT is §15.18, Feature T191; the view PERIODS is this implementation's own.
System-versioned tables are §4.17.2.3 and §11.3, Feature T180, the
transaction timestamp's type TIMESTAMP(6) WITH TIME ZONE (IV072);
TABLE_TYPE SYSTEM VERSIONED is an extension. That AS is not written in a CREATE SCHEMA, and
that neither names a declared local temporary table, are this
implementation's own. The null treatment of UNIQUE is ISO/IEC
9075-2:2023 §11.7, Feature F292. With none written it is NULLS DISTINCT,
and NULLS NOT DISTINCT takes two rows all null as the same, both choices
the standard leaves to the implementation. Outside a `<schema definition>`, CREATE TABLE is SQL-92's
`<SQL schema statement>` [§20.1], and a constraint's name written without
a schema is of the default schema [§5.4 SR8b]. USING and WITH are
extensions. AS NODE, AS EDGE, the pseudo-columns and CONNECTION
constraints are the property graph implementation standard's Mode B,
§4.2, R5.1 for the identities and R12.1 and R12.2 for a node's edges and
CONNECTION; the views GRAPH_TABLES and CONNECTION_CONSTRAINTS and the
column IS_HIDDEN are this implementation's own. PARTITION BY and
PARTITION OF are extensions. The XML options of a
CHECK and of a generation expression are ISO/IEC 9075-14:2006 draft
§12.1 and §12.2, Features X083 and X133.

## SEE ALSO

[ALTER TABLE](alter-table.md), [DROP TABLE](drop-table.md),
[CREATE DOMAIN](create-domain.md), [CREATE INDEX](create-index.md),
[CREATE SEQUENCE](create-sequence.md), [INSERT](insert.md),
[CREATE PROPERTY GRAPH](create-property-graph.md),
[Partitioned Tables](../23-partitioning.md),
[Constraints](../../user/07-constraints.md)
