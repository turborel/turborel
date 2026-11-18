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
    | DISTANCE <schema name> . <distance function name>
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

`DISTANCE s.f` names a distance function registered to the database
under the name `S.F` as the metric. The index measures by it, and
`VECTOR_DISTANCE(v, q, 'S.F')` is served by it. A program registers one
with `Database::register_metric`.

Each of `diskann`, `hnsw` and `ivf` declares TARGET_ACCURACY, PQ_BYTES
and NORMALIZED in its own group. TARGET_ACCURACY, 90 by default, is the
share of the true nearest rows a search aims at when the query names no
accuracy or parameters of its own. Setting the method's own search list,
EF_SEARCH or PROBES makes the index search by that instead. PQ_BYTES is
the bytes of a quantizer's code, 0 for its default. DiskANN's 0 with the
built-in PQ is half the dimensions, a byte for each two, as codes any
coarser guide its search poorly. NORMALIZED ON
declares every vector of the column of norm 1, and a row whose vector is
not is refused with SQLCODE -301.

The index serves a query whose ORDER BY is a distance of its column of
the index's own metric. EUCLIDEAN and EUCLIDEAN_SQUARED serve each
other. On vectors of norm 1, COSINE, DOT, EUCLIDEAN and EUCLIDEAN_SQUARED
order rows alike, so an index whose NORMALIZED is ON serves any of them,
whichever is its own. A graph of `diskann` or `hnsw` needs a metric that
is never negative, so its DOT or COSINE is measured by EUCLIDEAN when
NORMALIZED is ON, and refused when it is OFF. The setting
VECTOR.BUILD_MEMORY_BYTES bounds the memory a build takes.

A vector method makes no UNIQUE index, as vectors are compared by no
`=`. There is no CREATE UNIQUE INDEX, and a UNIQUE constraint whose
TABLES.UNIQUE_INDEX_METHOD names a vector method is refused with
SQLCODE -900.

ALTER INDEX REBUILD builds the index again, with the parameters given
set first, each named as CREATE VECTOR INDEX names it. MERGE DELTA runs
now the maintenance the index otherwise leaves to the database's
maintenance job, such as merging the rows changed since it was built.
A DiskANN index grown row by row also trains its codes then, once it
holds DISKANN.TRAIN_AT rows, and caches again the nodes its searches
pass first each time its rows double.
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
| 22000 | -301 | A row's vector is not of norm 1, and an index of its column has NORMALIZED ON |

## EXAMPLES

Of a table DOCS (ID INTEGER, EMBEDDING VECTOR(3, FLOAT32)) of four rows:

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

The nearest rows by COSINE, found by DOCS_HNSW:

```sql
SELECT ID FROM DOCS ORDER BY VECTOR_DISTANCE(EMBEDDING, '[1,0,0]', 'COSINE') FETCH APPROX FIRST 2 ROWS ONLY;
EXPLAIN SELECT ID FROM DOCS ORDER BY VECTOR_DISTANCE(EMBEDDING, '[1,0,0]', 'COSINE') FETCH APPROX FIRST 2 ROWS ONLY;
```

```
           ID
--------------
            1
            2
(2 rows)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project DOCS.ID
   Limit fetch 2 approx
     Sort VECTOR_DISTANCE(DOCS.EMBEDDING, ASSIGN('[1,0,0]'), ASSIGN('COSINE'))
       Project DOCS.ID, VECTOR_DISTANCE(DOCS.EMBEDDING, ASSIGN('[1,0,0]'), ASSIGN('COSINE'))
         Scan ANN.DOCS
 -- rules: constant_folding
 -- optimized:
 Project DOCS.ID
   Limit fetch 2 approx
     Sort VECTOR_DISTANCE(DOCS.EMBEDDING, TO_VECTOR('[1,0,0]', 3, 'FLOAT32'), 'COSINE')
       Project DOCS.ID, VECTOR_DISTANCE(DOCS.EMBEDDING, TO_VECTOR('[1,0,0]', 3, 'FLOAT32'), 'COSINE')
         Scan ANN.DOCS
 -- physical:
 Project DOCS.ID
   Limit fetch 2 approx
     Sort VECTOR_DISTANCE(DOCS.EMBEDDING, TO_VECTOR('[1,0,0]', 3, 'FLOAT32'), 'COSINE')
       Project DOCS.ID, VECTOR_DISTANCE(DOCS.EMBEDDING, TO_VECTOR('[1,0,0]', 3, 'FLOAT32'), 'COSINE')
         NearestScan ANN.DOCS using DOCS_HNSW [hnsw]: VECTOR_DISTANCE(DOCS.EMBEDDING, TO_VECTOR('[1,0,0]', 3, 'FLOAT32'), 'COSINE')  (first=2, cost=31.4; approximate COSINE post-filter, target accuracy=index, search list=index, beam width=index, probes=index, rerank=every candidate by exact distance)
(19 rows)
```

Of a table UNIT (ID INTEGER, V VECTOR(2, FLOAT32)) of vectors of norm 1,
an index of DOT declared NORMALIZED serves COSINE, and refuses a vector
of another norm:

```sql
COMMIT;
CREATE VECTOR INDEX UNIT_V ON UNIT (V) USING hnsw WITH (METRIC = 'DOT', HNSW.NORMALIZED = 'ON');
SELECT ID FROM UNIT ORDER BY COSINE_DISTANCE(V, '[0.8,0.6]') FETCH APPROX FIRST 2 ROWS ONLY;
INSERT INTO UNIT VALUES (4, '[3,4]');
```

```
OK
OK
           ID
--------------
            2
            1
(2 rows)
turborel: SQLCODE -301 (SQLSTATE 22000): a hnsw index declares its column NORMALIZED (R13.1), and takes no vector whose norm is not 1
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
