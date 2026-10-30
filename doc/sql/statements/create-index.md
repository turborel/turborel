# CREATE INDEX

[Statements](README.md)

## NAME

CREATE INDEX: make an index on a table *(extension)*

## SYNOPSIS

```
<create index statement> ::=
    CREATE INDEX <index name> ON <table name>
    ( <column name> [ { , <column name> }... ] )
    [ USING <method name> ]
    [ WITH ( <index option> [ { , <index option> }... ] ) ]

<index option> ::= <option name> = { <option value> | DEFAULT }
```

## DESCRIPTION

CREATE INDEX makes an index on columns of a base table, kept by an index
method: `btree`, a B-link tree, the default; `hash`, a linear hash table;
or a method a plug-in registers. The planner uses an index when its cost
says the index is the cheaper way to find a query's rows: by the
comparisons of its columns with constants in a WHERE clause, BETWEEN,
IN lists of constants, ORs of these of one column, and LIKE patterns
that begin with characters; `hash` by equalities and IN lists alone. A
part of a WHERE clause that can raise an error, as a division can, is
evaluated on every row of the tables it reads, which are then read
whole, so the error is raised whatever the plan. An index changes no
result: only how fast it comes.

The index is built from the rows there now, while other transactions go
on reading and writing the table; it is then kept up to date with each
change. It is made at once, apart from any transaction: no ROLLBACK
undoes it.

An index's name is an identifier, not of a schema: no two indexes of the
database have one name. The options WITH sets are those the method
declares and those plug-ins declare for every index; the built-in
methods declare none. INFORMATION_SCHEMA.INDEXES lists the indexes, and
INDEX_OPTIONS their options.

An index takes no UNIQUE: a UNIQUE or PRIMARY KEY constraint makes an
index of its own, named for it, as `ANN.STAFF.UNIQUE1`.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The table is a base table, not a view, and its user owns the table's
   schema.
3. Each column is the table's.
4. No index has the name.
5. The method is registered; its name is matched in any case.
6. Each option is declared, given once, of a value it accepts.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the table's schema |

## EXAMPLES

Of a table STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), DEPT
CHAR(3)):

```sql
CREATE INDEX STAFF_DEPT ON STAFF (DEPT);
CREATE INDEX STAFF_NAME ON STAFF (NAME) USING HASH;
SELECT INDEX_NAME, METHOD_NAME, IS_UNIQUE, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES
    WHERE TABLE_NAME = 'STAFF' ORDER BY INDEX_NAME;
```

```
OK
OK
 INDEX_NAME                                                   | METHOD_NAME                                                  | IS_UNIQUE                                                    | COLUMN_NAMES
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ANN.STAFF.UNIQUE1                                            | btree                                                        | YES                                                          | ID
 STAFF_DEPT                                                   | btree                                                        | NO                                                           | DEPT
 STAFF_NAME                                                   | hash                                                         | NO                                                           | NAME
(3 rows)
```

Indexes refused, V a view:

```sql
CREATE INDEX STAFF_X ON STAFF (NOPE);
CREATE INDEX STAFF_X ON STAFF (DEPT) USING NOPE;
CREATE INDEX STAFF_DEPT ON STAFF (DEPT);
CREATE INDEX STAFF_X ON V (ID);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.STAFF has no column NOPE (at character 31)
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no index method NOPE (at character 43)
turborel: SQLCODE -103 (SQLSTATE 2A000): an index named STAFF_DEPT already exists (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.V is not a base table (at character 24)
```

A session with a transaction open:

```sql
SELECT 1 FROM STAFF WHERE ID = 1;
CREATE INDEX STAFF_Y ON STAFF (DEPT);
```

```
-----
   1
(1 row)
turborel: SQLCODE -917 (SQLSTATE 25000): an index statement runs outside a transaction: commit or roll back first
```

## STANDARD

An extension: neither SQL-86 nor SQL-92 has indexes.

## SEE ALSO

[ALTER INDEX](alter-index.md), [DROP INDEX](drop-index.md),
[CREATE TABLE](create-table.md)
