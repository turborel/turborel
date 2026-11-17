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
`rtree`, a spatial tree over coordinate columns or fixed-SRID geometries;
`fulltext`, an inverted index of a text column's words; or a method a
plug-in registers. The planner uses an index when its cost
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
an index cannot hide its error (D15). This Boolean function contract is
separate from the built-in geometry lookup described below. See
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
the complete join condition.

An `rtree` can also index one `GEOMETRY(srid)` column. It stores the
geometry's envelope and can supply candidates for
`ST_INTERSECTS(column, value) = 1`, where `value` is a constant or parameter.
The executor fetches the visible geometry and checks the full condition;
an envelope match alone never establishes intersection. The column must
enforce one SRID. NULLs are omitted and empty values never intersect.
Inner joins can also probe it when both geometry columns enforce the same
SRID; the full ON condition checks every candidate pair. Ascending
ST_DISTANCE with FETCH FIRST can also use this index, refining each
candidate's geometry before deciding which rows to return. Geometry outer
joins and distance filters are not indexed yet. See the
[spatial guide](../../data/spatial/03-indexing.md) and
[distance queries](../../data/spatial/05-distance.md) for runnable examples.

A `fulltext` index takes one FULLTEXT column, or one character string
column with no collation. It keeps each word of the column's text and
the rows holding it, and supplies the rows for `CONTAINS(column, value)
= 1` and `TEXT_MATCHES(column, value)`, where `value` is a constant or
parameter, and the best ranked rows of a Contains filter ordered by
`RANK(column, value) DESC` with FETCH FIRST. The executor checks each
row it finds. Its options are FULLTEXT.LANGUAGE, FULLTEXT.STOP_WORDS,
FULLTEXT.K1, FULLTEXT.B and FULLTEXT.RANKING, which names BM25 or a
ranking the program registered. See [Full-Text Search](../fulltext.md) for its rules and the
[full-text guide](../../data/fulltext/README.md) for runnable examples.

The index is built from the rows there now, while other transactions go
on reading and writing the table; it is then kept up to date with each
change. It is made at once, apart from any transaction: no ROLLBACK
undoes it.

An index's name is an identifier, not of a schema: no two indexes of the
database have one name. The options WITH sets are those the method
declares and those plug-ins declare for every index; of the built-in
methods, only `fulltext` declares any. INFORMATION_SCHEMA.INDEXES lists the indexes, and
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
   temporary table, it is `btree`, `hash`, `rtree` or `fulltext`.
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
| 0A000 | -900 | `fulltext` is asked to index other than one FULLTEXT column or one character string column with no collation |

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

A dispatch map with numeric courier coordinates. Index X and Y in that
order, then ask for couriers in the visible window:

```sql
COMMIT;
CREATE TABLE COURIERS (
    ID INTEGER PRIMARY KEY,
    NAME VARCHAR(12),
    X DOUBLE PRECISION NOT NULL,
    Y DOUBLE PRECISION NOT NULL
);
INSERT INTO COURIERS VALUES
    (1, 'Amina', 100, 100), (2, 'Bo', 750, 250), (3, 'Chao', 1500, 200);
COMMIT;
CREATE INDEX COURIERS_XY ON COURIERS (X, Y) USING RTREE;
SELECT NAME FROM COURIERS
WHERE X BETWEEN 0 AND 1000 AND Y BETWEEN 0 AND 400
ORDER BY ID;
```

```
OK
OK
OK
OK
OK
 NAME
--------------
 Amina
 Bo
(2 rows)
```

Use the same coordinates to find each courier's dispatch area. These
areas are rectangles in the same local metre grid; a courier on their
shared edge would match both:

```sql
SELECT C.NAME, A.AREA_NAME
FROM COURIERS C JOIN
    (VALUES ('West', 0E0, 0E0, 1000E0, 400E0),
            ('East', 1000E0, 0E0, 2000E0, 400E0))
    AS A (AREA_NAME, MIN_X, MIN_Y, MAX_X, MAX_Y)
ON C.X >= A.MIN_X AND C.X <= A.MAX_X
AND C.Y >= A.MIN_Y AND C.Y <= A.MAX_Y
ORDER BY C.ID, A.AREA_NAME;
```

```
 NAME         | AREA_NAME
--------------+-----------
 Amina        | West
 Bo           | West
 Chao         | East
(3 rows)
```

For geometry locations, index the single fixed-SRID column instead. This
example creates a depot index and finds depots in a map window. It assumes
`turboreld`, which includes the geometry type and functions:

```sql
COMMIT;
SET SQL.FUNCTION_CALLS = 'ON';
CREATE TABLE PICKUP_DEPOTS (
    ID INTEGER PRIMARY KEY,
    NAME VARCHAR(12),
    LOCATION GEOMETRY(0)
);
INSERT INTO PICKUP_DEPOTS VALUES
    (1, 'Canal depot', ST_POINT(100, 700)),
    (2, 'Market depot', ST_POINT(1000, 300));
COMMIT;
CREATE INDEX PICKUP_DEPOTS_LOCATION ON PICKUP_DEPOTS (LOCATION) USING RTREE;
SELECT NAME FROM PICKUP_DEPOTS
WHERE ST_INTERSECTS(LOCATION, ST_MAKEENVELOPE(0, 0, 500, 1000)) = 1
ORDER BY ID;
```

```
OK
OK
OK
OK
OK
OK
 NAME
--------------
 Canal depot
(1 row)
```

These small tables can still use scans; index choice is based on cost.
See the [dispatch guide](../../data/spatial/README.md) for service-area
joins and nearest-depot searches using the same query patterns.

A full-text index of notes, its stop words its own:

```sql
CREATE TABLE NOTES (ID INTEGER PRIMARY KEY, BODY VARCHAR(200));
INSERT INTO NOTES VALUES (1, 'Call the depot about the late van'), (2, 'Van serviced, depot informed');
COMMIT;
CREATE INDEX NOTES_BODY ON NOTES (BODY) USING FULLTEXT WITH (FULLTEXT.STOP_WORDS = 'the, about');
SELECT ID FROM NOTES WHERE TEXT_MATCHES(BODY, 'depot van') ORDER BY ID;
```

```
OK
OK
OK
OK
           ID
--------------
            1
            2
(2 rows)
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
[CREATE TABLE](create-table.md),
[R-tree queries](../../data/spatial/03-indexing.md),
[Full-Text Search](../fulltext.md)
