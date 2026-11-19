# CREATE INDEX

[Statements](README.md)

## NAME

CREATE INDEX: make an index on a table *(extension)*

## SYNOPSIS

```
<create index statement> ::=
    CREATE INDEX <index name> ON <table name>
    ( <key part> [ { , <key part> }... ] )
    [ USING <method name> ]
    [ WITH ( <index option> [ { , <index option> }... ] ) ]

<key part> ::= <column name> | ( <value expression> )

<index option> ::= <option name> = { <option value> | DEFAULT }
```

## DESCRIPTION

CREATE INDEX makes an index on columns of a base table, kept by an index
method: `btree`, a B-link tree, the default; `hash`, a linear hash table;
`rtree`, a spatial tree over coordinate columns or fixed-SRID geometries;
`fulltext`, an inverted index of a text column's words; `json`, an
inverted index of a column's JSON documents' paths and values;
`xml_path`, `xml_value`, `xml_property` or `xml_primary`, of an XML
column's paths and values; or a method a plug-in registers. The planner uses an index when its cost
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
`ST_INTERSECTS(column, value) = 1`, where `value` is a constant or parameter,
and for the other relationships true only of values that meet, ST_RELATE
of a pattern asking them to meet among them.
The executor fetches the visible geometry and checks the full condition;
an envelope match alone never establishes intersection. The column must
enforce one SRID. NULLs are omitted and empty values never intersect.
`ST_DISTANCE(column, value [, unit]) <= limit`, or `<`, reads the
envelopes within the limit. Inner and outer joins, LEFT, RIGHT and FULL,
can also probe it from the other operand's geometry; the full ON
condition checks every candidate pair, and an outer join keeps the rows
no probe matched, with nulls. ST_DISTANCE ordered with FETCH FIRST,
ascending or descending, with a unit or without, can also use this
index, refining each
candidate's geometry before deciding which rows to return. See
[Spatial Data](../10-spatial.md#the-rtree-index-method) for its rules and the
[Spatial Guide](../../data/02-spatial.md) for runnable examples.

An `rtree` made over rows already in the table is built whole and packed
from the root down, its nodes nine tenths full, its sorts spilling past
MEMORY.WORK_BYTES. Rows written while it is built are kept. A node that
deletes leave empty or under two fifths full is taken out or merged, the
nodes above it too when that leaves them sparse, and its space reused
once no query can still read it. The index grows shorter as it empties
(RULES.md D139). See
[Spatial Data](../10-spatial.md#the-rtree-index-method).

A `fulltext` index takes one FULLTEXT column, or one character string
column with no collation. It keeps each word of the column's text and
the rows holding it, and supplies the rows for `CONTAINS(column, value)
= 1` and `TEXT_MATCHES(column, value)`, where `value` is a constant or
parameter, and the best ranked rows of a Contains filter ordered by
`RANK(column, value) DESC` with FETCH FIRST. The executor checks each
row it finds. Its options are FULLTEXT.LANGUAGE, FULLTEXT.STOP_WORDS,
FULLTEXT.K1, FULLTEXT.B and FULLTEXT.RANKING, which names BM25 or a
ranking the program registered. See [Full-Text Search](../08-fulltext.md) for its rules and the
[full-text guide](../../data/01-fulltext.md) for runnable examples.

A `json` index takes one column of JSON documents, a character string
with no collation or of type JSON. It answers JSON_EXISTS, JSON_VALUE
compared equal to a constant or parameter, and JSON_CONTAINS, of a call
of the column whose other arguments are constants or parameters and
that gives null or false on an error, never ERROR ON ERROR. It may find
more documents than the call holds of, and each row it finds is checked
by the whole WHERE clause. A document that is no JSON text is found by
every lookup, so its error is raised as a scan raises it. Its options
are JSON.PATHS_ONLY, ON or OFF, the default, which keeps paths and no
values, and JSON.MAX_DEPTH, 0 to 128, the default, the most member
names of a path it keeps; changing either makes it anew. See the
[JSON guide](../../data/03-json.md#indexes) for runnable examples.

An `xml_path`, `xml_value`, `xml_property` or `xml_primary` index keys
one XML column: each element and attribute of its values, by its path,
its value as a string and as a number, and its place in document order,
each method in its own order ([XML indexes](../19-xml.md#xml-indexes));
`xml_primary` keeps each node's value whole up to 512 octets, so it
answers of a row from the index alone. No comparison of SQL reads it, as
XML values are not comparable: the planner reads it for the paths of
XMLEXISTS, XMLQUERY and XMLTABLE ([Queries an XML index reads
for](../19-xml.md#queries-an-xml-index-reads-for)). A value with more
elements and attributes than it keys,
or a name or path longer than the database's page size holds, is refused
as it is written.

The index is built from the rows there now, while other transactions go
on reading and writing the table; it is then kept up to date with each
change. It is made at once, apart from any transaction: no ROLLBACK
undoes it.

An index's name is an identifier, not of a schema: no two indexes of the
database have one name. The options WITH sets are those the method
declares and those plug-ins declare for every index; of the built-in
methods, only `fulltext` and `json` declare any. INFORMATION_SCHEMA.INDEXES lists the indexes, and
INDEX_OPTIONS their options.

A key part may be an expression in parentheses, of the table's columns
alone. Such an index is kept of each row's value of the expression, and
the planner reads it for a query's expression of the same form: one
written the same way, of the same columns and constants, so `A + B`
and not `B + A`. It is probed by `=`, a range, BETWEEN and IN of the
expression; a `btree` index gives ORDER BY of it its order, and so a
GROUP BY ordered by it, when the expression is never null, as a sum of
columns NOT NULL is. EXPLAIN shows the expression it probes, and
INDEXES the key part as written, in parentheses. An UPDATE that changes
no column the expression reads leaves its entry as it is.

An expression is a key part only if it can raise no error of any row
and gives the same value of the same row every time (RULES.md
D92): a column, a constant, a negation, a sum, difference or
product of exact numbers whose every value fits 18 digits, a CASE of
such, UPPER and LOWER of a character string, and JSON_VALUE,
JSON_QUERY and JSON_EXISTS of a character string or JSON column that
give null or a constant on an error, read no time zone (no `datetime()`,
`time_tz()` or `timestamp_tz()` in the path) and return no type with
one. A quotient, a CAST, a subquery, CURRENT_DATE, a sum that can
overflow, or ERROR ON ERROR is refused. So a lookup by the expression
skips no row whose error a scan would raise. Only `btree` and `hash`
take an expression. A column the expression reads cannot be renamed;
dropped, it drops the index with it.

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
3. Each column is the table's, and each expression reads the table's
   columns alone, can raise no error and is deterministic.
4. No index has the name.
5. The method is registered; its name is matched in any case. On a
   temporary table, it is `btree`, `hash`, `rtree`, `fulltext` or `json`.
   Of a key part that is an expression, it is `btree` or `hash`.
6. The columns are of types the method keys: of `xml_path`, `xml_value`,
   `xml_property` and `xml_primary`, one column of an XML type; of the others, types
   whose values compare, so not of an XML type.
7. Each option is declared, given once, of a value it accepts.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | A rule above is broken, as by an expression that can raise an error |
| 2A000 | -104 | Its user does not own the table's schema |
| 0A000 | -900 | `rtree` is asked to index an unsupported column count or type |
| 0A000 | -900 | A value of the table is past what an XML index keys |
| TB910 | -910 | An `rtree` coordinate is non-finite or a rectangle has reversed bounds |
| 0A000 | -900 | `fulltext` is asked to index other than one FULLTEXT column or one character string column with no collation |
| 0A000 | -900 | `json` is asked to index other than one character string column with no collation or one JSON column |

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

A courier history of ten thousand positions, indexed after it is
loaded, so the index is packed whole, then most of it deleted, which
gives the emptied nodes' space back:

```sql
COMMIT;
CREATE TABLE TRACKS (ID INTEGER PRIMARY KEY, X DOUBLE PRECISION NOT NULL, Y DOUBLE PRECISION NOT NULL);
INSERT INTO TRACKS
    WITH RECURSIVE N (I, X, Y) AS (
        VALUES (CAST(0 AS INTEGER), CAST(0 AS INTEGER), CAST(0 AS INTEGER))
        UNION ALL SELECT I + 1, CASE WHEN Y = 99 THEN X + 1 ELSE X END, CASE WHEN Y = 99 THEN 0 ELSE Y + 1 END
        FROM N WHERE I < 9999)
    SELECT I, X, Y FROM N;
COMMIT;
CREATE INDEX TRACKS_XY ON TRACKS (X, Y) USING RTREE;
SELECT COUNT(*) AS SEEN FROM TRACKS WHERE X BETWEEN 10 AND 14 AND Y BETWEEN 0 AND 9;
DELETE FROM TRACKS WHERE X < 90;
COMMIT;
SELECT COUNT(*) AS SEEN FROM TRACKS WHERE X BETWEEN 88 AND 92 AND Y BETWEEN 0 AND 9;
```

```
OK
OK
OK
OK
OK
                 SEEN
----------------------
                   50
(1 row)
OK
OK
                 SEEN
----------------------
                   30
(1 row)
```

A GEOMETRY location is indexed by its one column, of a fixed SRID. A
depot index finds the depots in a map window:

```sql
COMMIT;
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
 NAME
--------------
 Canal depot
(1 row)
```

The same index finds the depots near a point, and those near each stop,
a stop near none kept with nulls:

```sql
SELECT NAME FROM PICKUP_DEPOTS WHERE ST_DISTANCE(LOCATION, ST_POINT(900, 300)) <= 150 ORDER BY ID;
CREATE TABLE STOPS (ID INTEGER PRIMARY KEY, SPOT GEOMETRY(0));
INSERT INTO STOPS VALUES (1, ST_POINT(120, 700)), (2, ST_POINT(500, 500));
SELECT S.ID, D.NAME FROM STOPS S LEFT JOIN PICKUP_DEPOTS D
    ON ST_DISTANCE(S.SPOT, D.LOCATION) < 50 ORDER BY S.ID;
```

```
 NAME
--------------
 Market depot
(1 row)
OK
OK
           ID | NAME
--------------+--------------
            1 | Canal depot
            2 | NULL
(2 rows)
```

The planner may still read so small a table whole, as it chooses by
cost. The [Spatial Guide](../../data/02-spatial.md) has service-area
joins and nearest-depot searches.

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

An XML index of a catalogue's entries, and one refused of a column not
of an XML type:

```sql
COMMIT;
CREATE TABLE CATALOGUE (ID INTEGER, ENTRY XML(DOCUMENT));
COMMIT;
CREATE INDEX CATALOGUE_PATHS ON CATALOGUE (ENTRY) USING XML_PATH;
CREATE INDEX CATALOGUE_IDS ON CATALOGUE (ID) USING XML_PATH;
SELECT INDEX_NAME, METHOD_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE TABLE_NAME = 'CATALOGUE';
```

```
OK
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): index CATALOGUE_IDS of method xml_path keys one column, of an XML type (RULES.md D106)
 INDEX_NAME                                                   | METHOD_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 CATALOGUE_PATHS                                              | xml_path
(1 row)
```

Indexes refused, V a view:

```sql
COMMIT;
CREATE INDEX STAFF_X ON STAFF (NOPE);
CREATE INDEX STAFF_X ON STAFF (DEPT) USING NOPE;
CREATE INDEX STAFF_DEPT ON STAFF (DEPT);
CREATE INDEX STAFF_X ON V (ID);
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.STAFF has no column NOPE (at character 31)
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no index method NOPE (at character 43)
turborel: SQLCODE -103 (SQLSTATE 2A000): an index named STAFF_DEPT already exists (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.V is not a base table (at character 24)
```

Indexes of expressions: a parcel's code in any case, its gross weight,
and the city of its label, a JSON text, or null if the label is none:

```sql
COMMIT;
CREATE TABLE PARCELS (
    ID INTEGER PRIMARY KEY,
    CODE VARCHAR(12),
    WEIGHT INTEGER NOT NULL,
    PACKING INTEGER NOT NULL,
    LABEL VARCHAR(200)
);
INSERT INTO PARCELS VALUES
    (1, 'ab-1', 120, 5, '{"city":"Oslo"}'),
    (2, 'AB-2', 80, 4, '{"city":"Bergen"}'),
    (3, 'cd-3', 200, 10, 'not json');
COMMIT;
CREATE INDEX PARCELS_CODE ON PARCELS ((UPPER(CODE)));
CREATE INDEX PARCELS_GROSS ON PARCELS ((WEIGHT + PACKING), ID);
CREATE INDEX PARCELS_CITY ON PARCELS
    ((JSON_VALUE(LABEL, 'lax $.city' NULL ON ERROR))) USING HASH;
SELECT ID FROM PARCELS WHERE UPPER(CODE) = 'AB-2';
SELECT ID, WEIGHT + PACKING AS GROSS FROM PARCELS ORDER BY WEIGHT + PACKING;
SELECT ID FROM PARCELS WHERE JSON_VALUE(LABEL, 'lax $.city' NULL ON ERROR) = 'Oslo';
SELECT INDEX_NAME, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES
    WHERE TABLE_NAME = 'PARCELS' AND IS_UNIQUE = 'NO' ORDER BY INDEX_NAME;
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
OK
           ID
--------------
            2
(1 row)
           ID |                GROSS
--------------+----------------------
            2 |                   84
            1 |                  125
            3 |                  210
(3 rows)
           ID
--------------
            1
(1 row)
 INDEX_NAME                                                   | COLUMN_NAMES
--------------------------------------------------------------+--------------------------------------------------------------
 PARCELS_CITY                                                 | (JSON_VALUE(LABEL, 'lax $.city' NULL ON ERROR))
 PARCELS_CODE                                                 | (UPPER(CODE))
 PARCELS_GROSS                                                | (WEIGHT + PACKING), ID
(3 rows)
OK
```

Expressions refused: a quotient raises an error of a row whose PACKING
is 0, and ERROR ON ERROR one of a label that is no JSON text:

```sql
CREATE INDEX PARCELS_X ON PARCELS ((WEIGHT / PACKING));
CREATE INDEX PARCELS_X ON PARCELS ((JSON_VALUE(LABEL, 'lax $.city' ERROR ON ERROR)));
CREATE INDEX PARCELS_X ON PARCELS ((UPPER(CODE))) USING RTREE;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): the key part (WEIGHT / PACKING) of index PARCELS_X: (WEIGHT / PACKING) can raise an error or is not deterministic, so it is no key part of an index (RULES.md D92) (at character 35)
turborel: SQLCODE -103 (SQLSTATE 2A000): the key part (JSON_VALUE(LABEL, 'lax $.city' ERROR ON ERROR)) of index PARCELS_X: JSON_VALUE 'lax $.city' RETURNING CHARACTER VARYING(32767) NULL ON EMPTY ERROR ON ERROR(LABEL) can raise an error or is not deterministic, so it is no key part of an index (RULES.md D92) (at character 35)
turborel: SQLCODE -103 (SQLSTATE 2A000): a rtree index has no key part that is an expression, only btree and hash do (RULES.md D92) (at character 35)
```

A session with a transaction open:

```sql
SELECT 1 FROM STAFF WHERE ID = 1;
CREATE INDEX STAFF_Y ON STAFF (DEPT);
```

```

-----
(0 rows)
turborel: SQLCODE -917 (SQLSTATE 25000): an index statement runs outside a transaction: commit or roll back first
```

## STANDARD

An extension: neither SQL-86 nor SQL-92 has indexes.

## SEE ALSO

[ALTER INDEX](alter-index.md), [DROP INDEX](drop-index.md),
[CREATE TABLE](create-table.md),
[Full-Text Search](../08-fulltext.md),
[Spatial Data](../10-spatial.md),
[XML indexes](../19-xml.md#xml-indexes)
