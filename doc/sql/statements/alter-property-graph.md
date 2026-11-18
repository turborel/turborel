# ALTER PROPERTY GRAPH

[Statements](README.md)

## NAME

ALTER PROPERTY GRAPH: add or drop a graph's element tables, labels and properties, or compile it

## SYNOPSIS

```
<alter property graph statement> ::=
    ALTER PROPERTY GRAPH <graph name> <alter property graph action>

<alter property graph action> ::=
      ADD { VERTEX | EDGE } TABLES ( <element table definition> [ { , <element table definition> }... ] )
    | DROP { VERTEX | EDGE } TABLES ( <element table alias> [ { , <element table alias> }... ] ) [ <drop behavior> ]
    | ALTER { VERTEX | EDGE } TABLE <element table alias> <alter element table action>
    | COMPILE

<alter element table action> ::=
      ADD <label and properties>
    | DROP LABEL <label name> [ <drop behavior> ]
    | ALTER LABEL <label name> ADD PROPERTIES ( <derived property> [ { , <derived property> }... ] )
    | ALTER LABEL <label name> DROP PROPERTIES ( <property name> [ { , <property name> }... ] ) [ <drop behavior> ]

<drop behavior> ::= CASCADE | RESTRICT
```

The `<element table definition>`, `<label and properties>` and
`<derived property>` are those of
[CREATE PROPERTY GRAPH](create-property-graph.md).

## DESCRIPTION

ALTER PROPERTY GRAPH changes a graph, which is then checked by every
rule of CREATE PROPERTY GRAPH, as if defined anew as it now is.

ADD VERTEX TABLES and ADD EDGE TABLES add element tables. DROP VERTEX
TABLES drops vertex tables; RESTRICT, the default, refuses to drop one
an edge table joins, and CASCADE drops those edge tables too. ALTER
VERTEX TABLE and ALTER EDGE TABLE add a label to an element table,
drop one, or add or drop properties of one. A label whose properties
are all dropped is left with none.

A change to a table a graph reads that breaks a rule of the graph,
dropping the key it takes, say, leaves the graph INVALID. It is kept,
but not queried until COMPILE checks it again, when the rules hold
once more. ALTER TABLE DROP COLUMN of a column a graph names is refused,
or with CASCADE drops the graph; a column a graph names is not renamed.

ALTER PROPERTY GRAPH runs within the session's transaction: ROLLBACK
undoes it.

## RULES

1. The graph exists, and its user owns its schema.
2. Each alias, label and property dropped or altered is the graph's.
3. A graph keeps one vertex table, and each element table one label.
4. The graph, as changed, keeps the rules of CREATE PROPERTY GRAPH.
5. The transaction is not READ ONLY.

## PRIVILEGES

The graph's schema is its user's, who holds SELECT on each element table.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken; the message names the condition, as GRF-0012 ([Appendix A](../../appendix/a-sqlstate.md)) |
| 2A000 | -104 | Its user does not own the graph's schema, or lacks SELECT on an element table |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the graph FRIENDS of [CREATE PROPERTY GRAPH](create-property-graph.md):

```sql
CREATE TABLE CITY (ID INTEGER NOT NULL, NAME VARCHAR(20), CONSTRAINT CITY_PK PRIMARY KEY (ID));
ALTER PROPERTY GRAPH FRIENDS ADD VERTEX TABLES (CITY AS TOWN);
ALTER PROPERTY GRAPH FRIENDS ALTER VERTEX TABLE PERSON ADD LABEL NAMED PROPERTIES (NAME);
ALTER PROPERTY GRAPH FRIENDS ALTER VERTEX TABLE TOWN ADD LABEL NAMED PROPERTIES (NAME);
ALTER PROPERTY GRAPH FRIENDS ALTER VERTEX TABLE PERSON ALTER LABEL PERSON DROP PROPERTIES (LOUD);
ALTER PROPERTY GRAPH FRIENDS ALTER VERTEX TABLE TOWN DROP LABEL NAMED;
SELECT ELEMENT_TABLE_ALIAS, LABEL_NAME FROM INFORMATION_SCHEMA.PG_LABELS ORDER BY 1, ORDINAL_POSITION;
ALTER PROPERTY GRAPH FRIENDS ALTER VERTEX TABLE TOWN ADD LABEL NAMED PROPERTIES (ID);
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
 ELEMENT_TABLE_ALIAS                                          | LABEL_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 KNOWS                                                        | KNOWS
 PERSON                                                       | PERSON
 PERSON                                                       | NAMED
 TOWN                                                         | TOWN
(4 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): GRF-0011 label NAMED has other properties on element table TOWN than on PERSON (R4.4) (at character 21)
OK
```

A graph left INVALID, then compiled:

```sql
ALTER TABLE CITY DROP CONSTRAINT CITY_PK CASCADE;
SELECT PROPERTY_GRAPH_NAME, IS_VALID FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS;
ALTER PROPERTY GRAPH FRIENDS COMPILE;
ALTER TABLE CITY ADD CONSTRAINT CITY_PK PRIMARY KEY (ID);
ALTER PROPERTY GRAPH FRIENDS COMPILE;
SELECT PROPERTY_GRAPH_NAME, IS_VALID FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS;
COMMIT;
```

```
OK
 PROPERTY_GRAPH_NAME                                          | IS_VALID
--------------------------------------------------------------+--------------------------------------------------------------
 FRIENDS                                                      | NO
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): KEY (ID) of ANN.CITY is no PRIMARY KEY or UNIQUE key of NOT NULL columns, as ENFORCED MODE needs (R4.3) (at character 21)
OK
OK
 PROPERTY_GRAPH_NAME                                          | IS_VALID
--------------------------------------------------------------+--------------------------------------------------------------
 FRIENDS                                                      | YES
(1 row)
OK
```

## STANDARD

ISO/IEC 9075-16:2023 (SQL/PGQ) defines the ADD and DROP of element
tables, labels and properties. turborel follows its implementation
standard's Mode A (§4.1, R4.7 of INVALID graphs and COMPILE), an
extension (RULES.md D-GRAPH-CATALOG).

## SEE ALSO

[CREATE PROPERTY GRAPH](create-property-graph.md),
[DROP PROPERTY GRAPH](drop-property-graph.md), [ALTER TABLE](alter-table.md)
