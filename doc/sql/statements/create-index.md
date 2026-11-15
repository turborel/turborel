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
`rtree`, a spatial tree over coordinate columns; or a method a plug-in
registers. The planner uses an index when its cost
says the index is the cheaper way to find a query's rows: by the
comparisons of its columns with constants in a WHERE clause, BETWEEN,
IN lists of constants, ORs of these of one column, and LIKE patterns
that begin with characters; `hash` by equalities and IN lists alone. A
`btree` index read for an ORDER BY of its columns, or for ranges of
them an OR or an IN list names, gives the rows in its order, so they
are not sorted again. A join whose condition equates an indexed column
with a column of the other table, an inner join or a LEFT outer join
whose right table is the indexed one, may look up each row of the other
table in the index, when they are few, rather than read the indexed
table whole. A part of a WHERE clause, or of an ON clause, that can
raise an error, as a division can, is evaluated on every row of the
tables it reads, which are then read whole, so the error is raised
whatever the plan. An index changes no result: only how fast it comes.

A plug-in method may also support a filter such as `WHERE f(column, ?)`.
The registered function takes two arguments, returns BOOLEAN or null and
uses an indexed column in either argument position. The other argument
is a constant or parameter. The method declares which functions it
supports; the planner chooses a lookup by cost, as it does for a
comparison (RULES.md D34).

The index may return extra rows, called candidates. The executor fetches
each visible row and checks the full WHERE condition before returning it.
Only functions that promise the same answer for the same inputs, no side
effects, no errors for accepted arguments and null for a null argument
can use this lookup. A filter that can raise keeps a full table read, so
an index cannot hide its error (D15). The built-in `btree`, `hash` and `rtree`
methods support comparisons; a plug-in supplies the function lookup. See
[Indexes for function calls](../../client/03-embedding.md#indexes-for-function-calls-extension)
for a runnable example and the Rust API.

An `rtree` index takes two REAL, FLOAT or DOUBLE PRECISION columns in
(X, Y) order for points, or four in (min X, min Y, max X, max Y) order
for rectangles. Coordinates must be finite; each minimum must be at most
its maximum. WHERE comparisons can constrain several coordinates at once,
including ranges on both X and Y. The index checks each comparison against
the stored coordinates and provides no SQL key order. It omits rows with
a NULL coordinate, so the planner uses it only when those rows cannot
answer the query. An inner join can probe the index with coordinate bounds
from each row of the other operand. Both table orders and projections of
the indexed table's columns are considered; candidate rows still satisfy
the complete join condition. It does not implement geometry types or
SQL/MM functions.

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

An index on a temporary table is one of each session's rows
([Temporary Tables](../../user/09-temporary-tables.md)): each session
makes its entries, in memory, of its own rows, as its next statement
begins. A session whose transaction had its rows of the table open then
uses the index only once that transaction ends; until then, a statement
that would read or write through it is refused (SQLSTATE TB911), and is
run again after COMMIT or ROLLBACK.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The table is a base table, not a view, and its user owns the table's
   schema.
3. Each column is the table's.
4. No index has the name.
5. The method is registered; its name is matched in any case. On a
   temporary table, it is `btree`, `hash` or `rtree`.
6. Each option is declared, given once, of a value it accepts.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the table's schema |
| 0A000 | -900 | `rtree` is asked to index an unsupported column count or type |
| TB910 | -910 | An `rtree` coordinate is non-finite or a rectangle has reversed bounds |

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

A point window, of a table PLACES (ID INTEGER, X DOUBLE PRECISION NOT
NULL, Y DOUBLE PRECISION NOT NULL):

```sql
CREATE INDEX PLACES_XY ON PLACES (X, Y) USING RTREE;
INSERT INTO PLACES VALUES (1, 0, 0), (2, 5, 5), (3, 10, 10);
COMMIT;
SELECT ID FROM PLACES
    WHERE X BETWEEN 0 AND 5 AND Y BETWEEN 0 AND 5 ORDER BY ID;
```

```
OK
OK
OK
           ID
--------------
            1
            2
(2 rows)
```

Joining those points to two windows includes a point on a shared edge in
both windows. The planner chooses index probes when their estimated cost
is lower than testing every pair:

```sql
SELECT P.ID, W.ID AS WINDOW_ID
FROM PLACES P JOIN
    (VALUES (10, 0E0, 0E0, 5E0, 5E0), (20, 5E0, 5E0, 10E0, 10E0))
    AS W (ID, LX, LY, HX, HY)
ON P.X >= W.LX AND P.X <= W.HX AND P.Y >= W.LY AND P.Y <= W.HY
ORDER BY P.ID, W.ID;
```

```
           ID | WINDOW_ID
--------------+-----------
            1 |        10
            2 |        10
            2 |        20
            3 |        20
(4 rows)
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
