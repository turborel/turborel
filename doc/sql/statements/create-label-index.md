# CREATE LABEL INDEX

[Statements](README.md)

## NAME

CREATE LABEL INDEX: number the elements of a property graph's element tables for label tests *(extension)*

## SYNOPSIS

```
<create label index statement> ::=
    CREATE LABEL INDEX [ <index name> ] ON GRAPH <graph name>
    [ { VERTEX | EDGE } TABLE <element table alias> ]
```

## DESCRIPTION

CREATE LABEL INDEX makes indexes of the `labels` method on the tables of
a property graph's element tables, each of its element table's key. Such
an index numbers its table's rows densely. A GRAPH_TABLE over the graph
puts the numbers of every indexed table side by side and keeps, for each
label, a bitmap of the elements carrying it, across all the tables that
share the label. A reached element's labels are then its bits, and the
vertices a path pattern starts at are found in the bitmaps of its first
node pattern's label expression, `%`, `!`, `&` and `|` alike, without a
scan of the tables' rows ([CREATE INDEX](create-index.md)).

A label belongs to a whole element table in a graph defined over tables,
so its bitmap holds a table's elements all or none. The label tests give
what the tables' labels give, and the gain is the start vertices found
without reading their rows.

With `VERTEX TABLE` or `EDGE TABLE`, one index is made, of the element
table the alias names, named as written or `GRAPH_ALIAS_LABELS`.
Without, one is made of each element table that is a base table, named
`GRAPH_ALIAS_LABELS`, or, if a name is written, `NAME_ALIAS`. Each is the
[CREATE INDEX](create-index.md) of the `labels` method of the element
table's key columns.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The graph exists, and an alias written is that of one of its element
   tables of the kind written (GRF-0012).
3. Each table indexed is a base table, and its user owns the table's
   schema.
4. Each key column of each table indexed is NOT NULL.
5. No index has the name of one made.

## PRIVILEGES

The schema of each table indexed is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 42000 | -102 | Neither VERTEX nor EDGE comes before TABLE |
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | Its user does not own a table's schema |
| 0A000 | -900 | A key column of a table indexed may be null |

## EXAMPLES

```sql
CREATE TABLE PERSON (ID INTEGER PRIMARY KEY, NAME VARCHAR(20));
CREATE TABLE PLACE (ID INTEGER PRIMARY KEY, NAME VARCHAR(20));
CREATE TABLE VISIT (ID INTEGER PRIMARY KEY, WHO INTEGER NOT NULL REFERENCES PERSON,
    PLACE_ID INTEGER NOT NULL REFERENCES PLACE);
INSERT INTO PERSON VALUES (1, 'Ada'), (2, 'Bo');
INSERT INTO PLACE VALUES (1, 'Oslo');
INSERT INTO VISIT VALUES (1, 1, 1);
CREATE PROPERTY GRAPH TRIPS
    VERTEX TABLES (PERSON LABEL TRAVELLER PROPERTIES (NAME), PLACE LABEL CITY PROPERTIES (NAME))
    EDGE TABLES (VISIT SOURCE KEY (WHO) REFERENCES PERSON (ID)
        DESTINATION KEY (PLACE_ID) REFERENCES PLACE (ID) LABEL VISITED NO PROPERTIES);
COMMIT;
CREATE LABEL INDEX ON GRAPH TRIPS;
SELECT INDEX_NAME, TABLE_NAME, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES WHERE METHOD_NAME = 'labels';
SELECT N FROM GRAPH_TABLE (TRIPS MATCH (x:!City) COLUMNS (x.NAME AS N)) ORDER BY N;
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
OK
OK
 INDEX_NAME                                                   | TABLE_NAME                                                   | COLUMN_NAMES
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 TRIPS_PERSON_LABELS                                          | PERSON                                                       | ID
 TRIPS_PLACE_LABELS                                           | PLACE                                                        | ID
 TRIPS_VISIT_LABELS                                           | VISIT                                                        | ID
(3 rows)
 N
----------------------
 Ada
 Bo
(2 rows)
OK
```

The graph's three tables each have a label index. The vertices not
labelled CITY are found in the bitmaps of the labels, the travellers.

## STANDARD

The property graph implementation standard has no such statement: it
is an extension (RULES.md D149).

## SEE ALSO

[CREATE INDEX](create-index.md),
[CREATE PROPERTY GRAPH](create-property-graph.md),
[CREATE REACHABILITY INDEX](create-reachability-index.md),
[DROP INDEX](drop-index.md)
