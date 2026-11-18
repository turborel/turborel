# CREATE GRAPH TYPE

[Statements](README.md)

## NAME

CREATE GRAPH TYPE: define the node and edge types a typed GQL graph holds

## SYNOPSIS

```
<create graph type statement> ::=
    CREATE GRAPH TYPE [ IF NOT EXISTS ] <graph type name> [ AS ]
      { <element type> [ { , <element type> }... ] }

<element type> ::= <node type> | <edge type>

<node type> ::= NODE [ TYPE ] <type name> [ <labels> ] [ <property types> ]

<edge type> ::=
    [ DIRECTED | UNDIRECTED ] EDGE [ TYPE ] <type name> [ <labels> ]
      [ <property types> ] CONNECTING ( <node type name> { -> | ~ } <node type name> )

<labels> ::= { LABEL | LABELS | : } <label name> [ { & <label name> }... ]

<property types> ::= { <property name> <value type> [ NOT NULL ] [ , ... ] }
```

## DESCRIPTION

CREATE GRAPH TYPE defines a graph type of Mode C of the property graph
implementation standard. Each node type and each edge type has its
labels, its type name alone when none is written, and its properties,
each of a value type. An edge type connects a node type to a node type.

A value type is an SQL data type or one of GQL's names for one. STRING
is CHARACTER VARYING(32767), BOOL is BOOLEAN, INT8 and INT16 are
SMALLINT, INT32 is INTEGER, INT64 is BIGINT, FLOAT32 is REAL, FLOAT64
is DOUBLE PRECISION and DATETIME is TIMESTAMP.

A graph type holds no data. [CREATE GRAPH](create-graph.md) ... TYPED
makes a graph of it, with a table for each of its types.

## RULES

1. No graph type of the name exists, but with IF NOT EXISTS.
2. Type names are distinct, and so are the label sets of the node types.
3. An edge type's endpoints are node types of the graph type.
4. The transaction is not READ ONLY.

## PRIVILEGES

The graph type's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE GRAPH TYPE SOCIAL {
  NODE PERSON {NAME STRING NOT NULL, AGE INT32},
  NODE CITY {NAME STRING},
  EDGE KNOWS {SINCE INT32} CONNECTING (PERSON -> PERSON),
  EDGE LIVES_IN CONNECTING (PERSON -> CITY)
};
SELECT GRAPH_TYPE_NAME FROM INFORMATION_SCHEMA.GRAPH_TYPES;
```

```
OK
 GRAPH_TYPE_NAME
--------------------------------------------------------------
 SOCIAL
(1 row)
```

## STANDARD

ISO/IEC 39075:2024 (GQL) defines graph types. turborel keeps one as its
definition's text, of Mode C of its property graph implementation
standard (§4.3, RULES.md D110).

## SEE ALSO

[CREATE GRAPH](create-graph.md), [DROP GRAPH TYPE](drop-graph-type.md),
[EXECUTE GQL](execute-gql.md)
