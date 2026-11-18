# CREATE PROPERTY GRAPH

[Statements](README.md)

## NAME

CREATE PROPERTY GRAPH: define a property graph over tables

## SYNOPSIS

```
<create property graph statement> ::=
    CREATE [ OR REPLACE ] PROPERTY GRAPH [ IF NOT EXISTS ] <graph name>
    VERTEX TABLES ( <element table definition> [ { , <element table definition> }... ] )
    [ EDGE TABLES ( <element table definition> [ { , <element table definition> }... ] ) ]
    [ OPTIONS ( { ENFORCED | TRUSTED } MODE ) ]
    [ WITH INVOKER RIGHTS ]

<element table definition> ::=
    <table name> [ AS <element table alias> ]
    [ KEY ( <column name list> ) ]
    [ [ UNDIRECTED ] SOURCE <endpoint> DESTINATION <endpoint> ]
    [ <label and properties>... | <element table properties> ]

<endpoint> ::=
      <element table alias>
    | KEY ( <column name list> ) REFERENCES <element table alias> [ ( <column name list> ) ]

<label and properties> ::= { LABEL <label name> | DEFAULT LABEL } [ <element table properties> ]

<element table properties> ::=
      NO PROPERTIES
    | PROPERTIES ARE ALL COLUMNS [ EXCEPT ( <column name list> ) ]
    | PROPERTIES ( <derived property> [ { , <derived property> }... ] )

<derived property> ::= <value expression> [ AS <property name> ]
```

SOURCE and DESTINATION are written for an edge table, and only for one.

## DESCRIPTION

CREATE PROPERTY GRAPH defines a graph whose vertices and edges are the
rows of tables. Each element table is a base table or a view, named in
the graph by its alias, its table's name if none is written. A vertex
table's rows are vertices. An edge table's rows are edges, each from
the vertex its SOURCE columns match to the one its DESTINATION columns
match, or between them both ways if UNDIRECTED. No data is copied, as the
graph reads its tables as they are.

An element table's KEY tells its rows apart. Without one, it is the
table's primary key, or else its one unique key of columns that are
NOT NULL. An endpoint written as an alias alone is joined by the one
foreign key from the edge table to that vertex table. With KEY and
REFERENCES, it is joined by the columns written, to the vertex table's
key if no columns follow the alias.

Each element table has labels, each with properties. A property is a
column of the table, or an expression of its columns, named with AS
where it is not a column alone. Written without labels, an element
table has one label, named after its alias, of all its columns, as
DEFAULT LABEL is. A label shared by element tables has the same
property names on each, and the types of a property under one label
are of one type all can be cast to.

ENFORCED MODE, the default, takes each key from one the table declares,
and each endpoint from a foreign key it declares. TRUSTED MODE takes
the keys and endpoints as written, the user vouching for them. An edge
whose endpoint matches no vertex is then passed over by a query.

The graph is its schema owner's. Defined with the owner's rights, a
query of it needs SELECT on the graph alone; WITH INVOKER RIGHTS, it
needs SELECT on each table it reads too.

OR REPLACE replaces a graph of the name, keeping the privileges granted
on it. IF NOT EXISTS leaves a graph of the name as it is.

CREATE PROPERTY GRAPH runs within the session's transaction: ROLLBACK
undoes it.

## RULES

1. The graph's schema exists, and its user owns it. A graph named
   without a schema is of the session's default schema.
2. No table, view or graph of the schema has the name, but with OR
   REPLACE a graph's, or with IF NOT EXISTS.
3. Each element table is a base table or a view, of its user's SELECT;
   no alias is written twice.
4. Each element table has a key, written or found; each edge has its
   endpoints' vertex tables among the graph's vertex tables, and its
   endpoint columns are as many as those they match, of comparable
   types.
5. Under ENFORCED MODE, each key is one the table declares, and each
   endpoint one of its foreign keys.
6. No label is written twice for one element table, and no property
   twice for one label. A property's expression reads no set function.
7. A label shared by element tables has the same property names on
   each, of types that have one type in common.
8. The transaction is not READ ONLY.

## PRIVILEGES

SELECT on each element table, held by the graph's owner. Its owner holds
SELECT on the graph, grantable if it holds SELECT grantable on each
table the graph reads.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -103 | A rule above is broken; the message names the condition, as GRF-0003 ([Appendix A](../../appendix/a-sqlstate.md)) |
| 2A000 | -104 | Its user does not own the schema, or lacks SELECT on an element table |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE TABLE PERSON (ID INTEGER PRIMARY KEY, NAME VARCHAR(20));
CREATE TABLE KNOWS (A INTEGER NOT NULL REFERENCES PERSON, B INTEGER NOT NULL REFERENCES PERSON, SINCE INTEGER, PRIMARY KEY (A, B));
CREATE PROPERTY GRAPH FRIENDS VERTEX TABLES (PERSON PROPERTIES (NAME, UPPER(NAME) AS LOUD)) EDGE TABLES (KNOWS SOURCE KEY (A) REFERENCES PERSON DESTINATION KEY (B) REFERENCES PERSON);
SELECT ELEMENT_TABLE_ALIAS, LABEL_NAME, PROPERTY_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.PG_PROPERTIES ORDER BY 1, ORDINAL_POSITION;
COMMIT;
```

```
OK
OK
OK
 ELEMENT_TABLE_ALIAS                                          | LABEL_NAME                                                   | PROPERTY_NAME                                                | DATA_TYPE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 KNOWS                                                        | KNOWS                                                        | A                                                            | INTEGER
 KNOWS                                                        | KNOWS                                                        | B                                                            | INTEGER
 KNOWS                                                        | KNOWS                                                        | SINCE                                                        | INTEGER
 PERSON                                                       | PERSON                                                       | NAME                                                         | CHARACTER VARYING(20)
 PERSON                                                       | PERSON                                                       | LOUD                                                         | CHARACTER VARYING(20)
(5 rows)
OK
```

Graphs refused:

```sql
CREATE PROPERTY GRAPH G VERTEX TABLES (PERSON) EDGE TABLES (KNOWS SOURCE PERSON DESTINATION PERSON);
CREATE PROPERTY GRAPH G VERTEX TABLES (PERSON KEY (NAME));
CREATE PROPERTY GRAPH G VERTEX TABLES (PERSON LABEL P PROPERTIES (UPPER(NAME)));
CREATE PROPERTY GRAPH PERSON VERTEX TABLES (PERSON);
CREATE PROPERTY GRAPH IF NOT EXISTS FRIENDS VERTEX TABLES (PERSON);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): GRF-0003 ANN.KNOWS has no one FOREIGN KEY to ANN.PERSON, so write SOURCE KEY (...) REFERENCES PERSON (R4.2) (at character 73)
turborel: SQLCODE -103 (SQLSTATE 2A000): KEY (NAME) of ANN.PERSON is no PRIMARY KEY or UNIQUE key of NOT NULL columns, as ENFORCED MODE needs (R4.3) (at character 39)
turborel: SQLCODE -103 (SQLSTATE 2A000): a property that is no column alone is named with AS (R4.5) (at character 66)
turborel: SQLCODE -103 (SQLSTATE 2A000): schema ANN has a table, view or property graph PERSON already, whose names are one namespace (at character 22)
OK
```

## STANDARD

ISO/IEC 9075-16:2023 (SQL/PGQ) defines the property graph over tables.
turborel follows its implementation standard's Mode A (§4.1, rules
R4.1 to R4.7, conditions of §22), whose OR REPLACE, IF NOT EXISTS,
OPTIONS, UNDIRECTED and WITH INVOKER RIGHTS are extensions (RULES.md
D-GRAPH-CATALOG).

## SEE ALSO

[ALTER PROPERTY GRAPH](alter-property-graph.md),
[DROP PROPERTY GRAPH](drop-property-graph.md), [GRANT](grant.md),
[CREATE VIEW](create-view.md)
