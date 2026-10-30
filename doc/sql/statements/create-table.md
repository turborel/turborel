# CREATE TABLE

[Statements](README.md)

## NAME

CREATE TABLE: define a base table

## SYNOPSIS

```
<table definition> ::=
    CREATE [ { GLOBAL | LOCAL } TEMPORARY ] TABLE <table name>
    ( <table element> [ { , <table element> }... ] )
    [ ON COMMIT { DELETE | PRESERVE } ROWS ]
    [ USING <engine name> ]
    [ WITH ( <table option> [ { , <table option> }... ] ) ]

<table element> ::= <column definition> | <table constraint definition>

<column definition> ::=
    <column name> { <data type> | <domain name> }
    [ DEFAULT <default option> ]
    [ <column constraint definition>... ]

<default option> ::=
      <literal>
    | USER | CURRENT_USER | SESSION_USER | SYSTEM_USER
    | CURRENT_DATE | CURRENT_TIME [ ( <precision> ) ] | CURRENT_TIMESTAMP [ ( <precision> ) ]
    | NULL

<column constraint definition> ::=
    [ CONSTRAINT <constraint name> ]
    { NOT NULL | UNIQUE | PRIMARY KEY | <references specification> | <check constraint definition> }
    [ <constraint attributes> ]

<table constraint definition> ::=
    [ CONSTRAINT <constraint name> ]
    {   { UNIQUE | PRIMARY KEY } ( <column name> [ { , <column name> }... ] )
      | FOREIGN KEY ( <column name> [ { , <column name> }... ] ) <references specification>
      | <check constraint definition> }
    [ <constraint attributes> ]

<references specification> ::=
    REFERENCES <table name> [ ( <column name> [ { , <column name> }... ] ) ]
    [ MATCH { FULL | PARTIAL } ]
    [ ON UPDATE <referential action> ] [ ON DELETE <referential action> ]

<referential action> ::= CASCADE | SET NULL | SET DEFAULT | NO ACTION

<check constraint definition> ::= CHECK ( <search condition> )

<constraint attributes> ::=
      INITIALLY { DEFERRED | IMMEDIATE } [ [ NOT ] DEFERRABLE ]
    | [ NOT ] DEFERRABLE [ INITIALLY { DEFERRED | IMMEDIATE } ]

<table option> ::= <option name> = { <option value> | DEFAULT }
```

ON UPDATE and ON DELETE may be written in either order, each once. The
data types are in [Data Types](../02-data-types.md); a column may also
be of a type a plug-in registers *(extension)*, written by its name and,
in parentheses, one or two unsigned integers it takes.

## DESCRIPTION

CREATE TABLE defines a base table: its columns, each of a data type or a
domain, its constraints, and, *(extension)*, the storage engine that
keeps its rows and that engine's options.

A column's default is its own, written DEFAULT, else its domain's, else
the null value. A datetime literal written without a time zone takes
the session's ([SET TIME ZONE](set-time-zone.md)); CURRENT_DATE and the
others are taken as the statement that inserts the row runs.

**Constraints.** NOT NULL refuses the null value. UNIQUE refuses two
rows of the same values in its columns, of rows none of whose values is
null. PRIMARY KEY is UNIQUE and NOT NULL, one to a table. A foreign key
refuses a row whose values do not match a row of the referenced table:
of its PRIMARY KEY, or of the columns named, which a UNIQUE or PRIMARY
KEY constraint is on. Its MATCH says how a row with nulls in the foreign
key's columns is taken: with none written, it is accepted if any of
them is null; MATCH FULL accepts it only if all are null, and refuses
one with some null and some not; MATCH PARTIAL accepts it if its values
that are not null match a row's. ON UPDATE and ON DELETE say what is done to the
rows that refer to one changed or deleted: refused (NO ACTION, the
default), changed or deleted with it (CASCADE), or their values set null
or to their defaults. A CHECK refuses a row for which its condition is
false; it may read other tables through subqueries.

A constraint is checked at the end of each statement, or, DEFERRABLE
INITIALLY DEFERRED or deferred by [SET CONSTRAINTS](set-constraints.md),
at commit. A constraint not named is given a name in the table's
schema: the table's, its kind, and a number, as `EMPLOYEES_PRIMARY1`. A
constraint's name written without a schema is of the session's default
schema, which may be another than the table's: in a session whose
default schema is ANN, `CREATE TABLE SHOP.ITEMS (ID INTEGER CONSTRAINT
KEYED PRIMARY KEY)` names its constraint ANN.KEYED. In a [CREATE
SCHEMA](create-schema.md), it is of the schema being defined.

**Temporary tables.** GLOBAL TEMPORARY or LOCAL TEMPORARY defines a
table whose rows are each session's own: empty as the session begins,
gone when it ends, and never recovered. ON COMMIT DELETE ROWS, the
default, empties them as each transaction ends; PRESERVE ROWS keeps
them. See [Temporary Tables](../../user/09-temporary-tables.md).

**Storage** *(extension)*. USING names the engine that keeps the table's
rows, else the database's setting DEFAULT_ENGINE; WITH sets the engine's
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

CREATE TABLE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The table's schema exists, and its user owns it. A table named without
   a schema is of the session's default schema.
2. No table or view of the schema has the name; no column is defined
   twice; the table has a column.
3. A constraint's name is new in its schema, which exists; in a CREATE
   SCHEMA, its schema is the one being defined; it is not of MODULE but
   for a declared local temporary table, whose constraints' are.
4. A table has at most one PRIMARY KEY, and no two unique constraints on
   the same columns.
5. A foreign key's referenced table is a base table, its referenced
   columns those of a unique constraint, as many as the referencing
   ones, each of the same type.
6. A CHECK contains no USER, CURRENT_USER, SESSION_USER or SYSTEM_USER,
   no CURRENT_DATE, CURRENT_TIME or CURRENT_TIMESTAMP, and is not
   possibly non-deterministic.
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

## PRIVILEGES

The table's schema is its user's. REFERENCES on each column a foreign
key references, and, for a CHECK, on each column it reads of another
table, or, if it names none, on one column of each table it reads.
USAGE on each domain a column is of.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist, or a constraint's does not |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or lacks REFERENCES or USAGE it needs |
| 25000 | -917 | The transaction is READ ONLY |
| 0A000 | -900 | A COLLATE or CHARACTER SET clause of another character set |

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

Tables refused:

```sql
CREATE TABLE T3 (A INTEGER PRIMARY KEY, B INTEGER PRIMARY KEY);
CREATE TABLE T5 (A INTEGER REFERENCES EMPLOYEES (NAME));
CREATE TABLE T9 (A INTEGER DEFAULT 'x');
CREATE TABLE T7 (A INTEGER) WITH (COLUMNSTORE.GROUP_ROWS = 4096);
CREATE TABLE T8 (A INTEGER) USING columnstore WITH (COLUMNSTORE.ORDER = 'B');
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.T3 has more than one PRIMARY KEY [ISO/IEC 9075:1992 §11.7 SR5, SR6] (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): the columns referenced are not those of a unique constraint of ANN.EMPLOYEES [ISO/IEC 9075:1992 §11.8 SR2a] (at character 27)
turborel: SQLCODE -103 (SQLSTATE 2A000): the default is not a <literal> of INTEGER [ISO/IEC 9075:1992 §11.5 SR3a] (at character 17)
turborel: SQLCODE -103 (SQLSTATE 2A000): rowstore takes no option COLUMNSTORE.GROUP_ROWS (at character 34)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.T8 has no column B, for the table option COLUMNSTORE.ORDER (at character 13)
```

## STANDARD

ISO/IEC 9075:1992 §11.3 to §11.9, and §10.6 for constraint names and
attributes; X3.135-1986 §6.2 to §6.4, whose tables have NOT NULL and
UNIQUE only. Outside a `<schema definition>`, CREATE TABLE is SQL-92's
`<SQL schema statement>` [§20.1], and a constraint's name written without
a schema is of the default schema [§5.4 SR8b]. USING and WITH are
extensions.

## SEE ALSO

[ALTER TABLE](alter-table.md), [DROP TABLE](drop-table.md),
[CREATE DOMAIN](create-domain.md), [CREATE INDEX](create-index.md),
[Constraints](../../user/07-constraints.md)
