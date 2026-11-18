# CREATE VECTOR INDEX

[Statements](README.md)

## NAME

CREATE VECTOR INDEX: make an approximate nearest-neighbour index on a
column *(extension)*

## SYNOPSIS

```
<create vector index statement> ::=
    CREATE VECTOR INDEX <index name> ON <table name> ( <column name> )
    { [ <vector index clause>... ]
    | [ USING <method name> ] WITH ( <vector index option> [ { , <vector index option> }... ] ) }

<vector index clause> ::=
      ORGANIZATION <organization>
    | DISTANCE <metric name>
    | WITH TARGET ACCURACY <unsigned integer>
    | PARAMETERS ( <parameter> [ { , <parameter> }... ] )
    | QUANTIZATION { NONE | SQ8 | BQ | PQ [ ( <unsigned integer> [ , 8 ] ) ] }
    | RERANK { ON | OFF }
    | LOCAL | GLOBAL | ONLINE | PARALLEL <unsigned integer>

<organization> ::=
      INMEMORY NEIGHBOR GRAPH
    | NEIGHBOR PARTITIONS
    | DISK NEIGHBOR GRAPH
    | INVERTED

<parameter> ::= TYPE <type name> | <parameter name> <value>

<vector index option> ::= <option name> = <option value>

<alter index statement> ::=
    ALTER INDEX <index name>
    { REBUILD [ ONLINE ] [ PARAMETERS ( <parameter> [ { , <parameter> }... ] ) ]
    | MERGE DELTA | LOAD | UNLOAD }
```

## DESCRIPTION

CREATE VECTOR INDEX makes an index that finds the rows nearest a query
vector approximately, for FETCH APPROX
([FETCH APPROX](../06-queries.md#fetch-approx-and-fetch-exact-extension)).
It is a [CREATE INDEX](create-index.md) of a vector method, with its
options named as the method names them, and is listed by
INFORMATION_SCHEMA.INDEXES and INDEX_OPTIONS as any index is.

Its method is named by ORGANIZATION, by a TYPE parameter, by TYPE in
the WITH form, or by USING. INMEMORY NEIGHBOR GRAPH and TYPE HNSW are
`hnsw`, a layered graph held in memory. NEIGHBOR PARTITIONS and TYPE
IVF are `ivf`, vectors clustered into partitions. DISK NEIGHBOR GRAPH
and TYPE DISKANN are `diskann`, a graph on disk with compressed vectors
in memory. INVERTED and TYPE INVERTED are `inverted`, posting lists of a
sparse vector's dimensions. With none of these the method is `diskann`.

Each other clause sets an option of the method, which is named by the
method in upper case, a period and the option. DISTANCE sets its METRIC,
WITH TARGET ACCURACY its TARGET_ACCURACY, QUANTIZATION its QUANTIZER, PQ
of m bytes also its PQ_BYTES, and RERANK its RERANK. Each parameter sets
the option of its own name. The names of other systems are taken too.
NEIGHBORS is `hnsw`'s M, EFCONSTRUCTION its EF_CONSTRUCTION, NEIGHBOR
PARTITIONS `ivf`'s LISTS, and SAMPLE_PER_PARTITION its SAMPLE_PER_LIST.
In the WITH form, METRIC, TARGET_ACCURACY and QUANTIZATION are taken the
same way, and a name with a period is the option as written. LOCAL,
GLOBAL, ONLINE, PARALLEL and the WITH form's MAXDOP and ONLINE are taken
and change nothing, since every index is built while the table stays
open to other transactions, as CREATE INDEX's is.

The index serves a query whose ORDER BY is a distance of its column of
the index's own metric. EUCLIDEAN and EUCLIDEAN_SQUARED serve each
other. The setting VECTOR.BUILD_MEMORY_BYTES bounds the memory a build
takes.

ALTER INDEX REBUILD builds the index again, with the parameters given
set first, each named as CREATE VECTOR INDEX names it. MERGE DELTA runs
now the maintenance the index otherwise leaves to the database's
maintenance job, such as merging the rows changed since it was built.
LOAD takes the index's structure into memory, and UNLOAD lets it go, for
a method that keeps it there. Each is made at once, apart from any
transaction.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The table is a base table, and its user owns the table's schema.
3. The column is the table's, of a type the method indexes.
4. No index has the name.
5. The method is registered. ORGANIZATION and TYPE, if both are written,
   name one method, as do a WITH form's TYPE and USING.
6. Each option is one the method declares, given once, of a value it
   accepts. PQ's second number, if written, is 8.
7. ALTER INDEX names an index that exists, whose table's schema its
   user owns.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the table's schema |
| 2A000 | -102 | An ORGANIZATION, a QUANTIZATION or an ALTER INDEX action is not one of those above |

## EXAMPLES

Of a table DOCS (ID INTEGER, EMBEDDING DOUBLE PRECISION) and the methods
`hnsw` and `diskann`:

```sql
CREATE VECTOR INDEX DOCS_HNSW ON DOCS (EMBEDDING)
    ORGANIZATION INMEMORY NEIGHBOR GRAPH DISTANCE COSINE WITH TARGET ACCURACY 95
    PARAMETERS (TYPE HNSW, NEIGHBORS 32, EFCONSTRUCTION 300);
CREATE VECTOR INDEX DOCS_DISK ON DOCS (EMBEDDING)
    WITH (TYPE = 'DiskANN', METRIC = 'euclidean', QUANTIZATION = 'PQ');
SELECT INDEX_NAME, METHOD_NAME, OPTION_NAME, OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS
    WHERE TABLE_NAME = 'DOCS' AND VALUE_SOURCE <> 'DEFAULT' ORDER BY INDEX_NAME, OPTION_NAME;
```

```
OK
OK
 INDEX_NAME                                                   | METHOD_NAME                                                  | OPTION_NAME                                                  | OPTION_VALUE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 DOCS_DISK                                                    | diskann                                                      | DISKANN.METRIC                                               | EUCLIDEAN
 DOCS_DISK                                                    | diskann                                                      | DISKANN.QUANTIZER                                            | PQ
 DOCS_HNSW                                                    | hnsw                                                         | HNSW.EF_CONSTRUCTION                                         | 300
 DOCS_HNSW                                                    | hnsw                                                         | HNSW.M                                                       | 32
 DOCS_HNSW                                                    | hnsw                                                         | HNSW.METRIC                                                  | COSINE
 DOCS_HNSW                                                    | hnsw                                                         | HNSW.TARGET_ACCURACY                                         | 95
(6 rows)
```

Build the graph again with more neighbours, and take it into memory:

```sql
COMMIT;
ALTER INDEX DOCS_HNSW REBUILD PARAMETERS (NEIGHBORS 48);
ALTER INDEX DOCS_HNSW LOAD;
SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS
    WHERE INDEX_NAME = 'DOCS_HNSW' AND OPTION_NAME = 'HNSW.M';
```

```
OK
OK
OK
 OPTION_VALUE
--------------------------------------------------------------
 48
(1 row)
```

An index is of one method:

```sql
CREATE VECTOR INDEX DOCS_IVF ON DOCS (EMBEDDING)
    ORGANIZATION NEIGHBOR PARTITIONS PARAMETERS (TYPE HNSW);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): the index is of one type: ivf or hnsw (at character 107)
```

## STANDARD

An extension: neither SQL-86 nor SQL-92 has indexes.

## SEE ALSO

[CREATE INDEX](create-index.md), [ALTER INDEX](alter-index.md),
[DROP INDEX](drop-index.md),
[FETCH APPROX](../06-queries.md#fetch-approx-and-fetch-exact-extension)
