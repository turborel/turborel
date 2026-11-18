# Hybrid Search *(extension)*

[Contents](README.md)

A hybrid search finds documents by their words and by their meaning at
once. A hybrid index of a table holds a full-text index of a text column
and a vector index of a column of embeddings, one row a document.
HYBRID_SEARCH reads the best candidates of each index, ranked by BM25
and by distance, and fuses the two lists into one. The design follows
§11 of Turborel's vector design document.

The examples on this page are run in order, by the user ANN, on a table
of help articles, each with an embedding of four dimensions:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE HELP (ID INTEGER PRIMARY KEY, BODY VARCHAR(200), EMB VECTOR(4));
INSERT INTO HELP VALUES
    (1, 'Refund policy for damaged items', TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32')),
    (2, 'Shipping times and delivery', TO_VECTOR('[0.9,0.1,0,0]', 4, 'FLOAT32')),
    (3, 'How to return a damaged parcel', TO_VECTOR('[0,1,0,0]', 4, 'FLOAT32')),
    (4, 'Refund of a gift card', TO_VECTOR('[0.1,0,1,0]', 4, 'FLOAT32')),
    (5, 'Store opening hours', TO_VECTOR('[0.7,0.7,0,0]', 4, 'FLOAT32'));
COMMIT;
```

```
OK
OK
OK
OK
```

## The hybrid index

```
<create hybrid index statement> ::=
    CREATE HYBRID VECTOR INDEX <index name> ON <table name>
        ( <text column name> , <vector column name> )
    [ PARAMETERS ( <hybrid parameter> [ { , <hybrid parameter> }... ] )
    | WITH ( <table option> [ { , <table option> }... ] ) ]

<hybrid parameter> ::=
      VECTOR_IDX <character string literal>
    | TEXT_IDX <character string literal>
```

The index is of the method `hybrid`. Its first column is a character
string, read as the [full-text index](08-fulltext.md) reads one. Its
second is a VECTOR column. The index holds a part of each: a full-text
index of the text and a vector index of the embeddings. A row whose
text is null is held by the vector part alone, and one whose embedding
is null by the text part alone, so each is still found by the other.

VECTOR_IDX's string is a list of parameters as
[CREATE VECTOR INDEX](statements/create-vector-index.md) takes them.
TYPE names the vector part's method: HNSW, the default, IVF, DISKANN,
or INVERTED for a sparse column. It sets the option
HYBRID.VECTOR_METHOD. DISTANCE sets the method's METRIC, and each other
parameter the method's option of its name. TEXT_IDX's string is a list
of words and values, each an option of the group FULLTEXT, so
`LANGUAGE english` sets FULLTEXT.LANGUAGE. In the WITH form each option
is written as CREATE INDEX writes it.

```sql
CREATE HYBRID VECTOR INDEX HELP_H ON HELP (BODY, EMB)
    PARAMETERS (VECTOR_IDX 'TYPE HNSW, DISTANCE COSINE, NEIGHBORS 8', TEXT_IDX 'LANGUAGE english');
SELECT METHOD_NAME, OPTION_NAME, OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS
    WHERE INDEX_NAME = 'HELP_H' AND VALUE_SOURCE <> 'DEFAULT' ORDER BY OPTION_NAME;
```

```
OK
 METHOD_NAME                                                  | OPTION_NAME                                                  | OPTION_VALUE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 hybrid                                                       | FULLTEXT.LANGUAGE                                            | ENGLISH
 hybrid                                                       | HNSW.M                                                       | 8
 hybrid                                                       | HNSW.METRIC                                                  | COSINE
 hybrid                                                       | HYBRID.VECTOR_METHOD                                         | HNSW
(4 rows)
```

The index embeds nothing itself. The design document's index takes a
MODEL and a CHUNKER to embed its text, and Turborel refuses both with
SQLCODE -900. Fill the vector column as any other.

Either part also serves an ordinary query, as an index of its method
would. A FETCH APPROX of a distance of the vector column, of the index's
metric, reads the vector part. A FETCH FIRST of RANK of the text
column, highest first, reads the text part.

## HYBRID_SEARCH

```
<hybrid search> ::=
    HYBRID_SEARCH ( <hybrid argument> [ { , <hybrid argument> }... ] )
    [ [ AS ] <correlation name> [ ( <derived column list> ) ] ]

<hybrid argument> ::=
      INDEX = <index name>
    | QUERY_TEXT = <value expression>
    | QUERY_VECTOR = <value expression>
    | TOP_N = <unsigned integer>
    | CANDIDATES = <unsigned integer>
    | FUSION = { 'RRF' | 'WEIGHTED' }
    | VECTOR_WEIGHT = <unsigned numeric literal>
    | TEXT_WEIGHT = <unsigned numeric literal>
    | RRF_K = <unsigned integer>
```

HYBRID_SEARCH is a table in a FROM clause. INDEX, QUERY_TEXT,
QUERY_VECTOR and TOP_N are written once each, and the others at most
once, in any order. The correlation name is HYBRID_SEARCH if none is
written.

| Argument | Is |
|---|---|
| INDEX | A hybrid index, of a table whose primary key is one column |
| QUERY_TEXT | The words searched for, each ranked as `IS ABOUT` ranks it ([Full-Text Patterns](09-fulltext-patterns.md)) |
| QUERY_VECTOR | The vector searched for, of the vector column's type |
| TOP_N | The rows it gives |
| CANDIDATES | The rows each list holds, TOP_N times 4 by default |
| FUSION | How the lists are fused, RRF by default |
| VECTOR_WEIGHT, TEXT_WEIGHT | Each list's weight, 1 by default |
| RRF_K | RRF's K, 60 by default |

The vector list is the CANDIDATES rows nearest QUERY_VECTOR by the
index's metric, read approximately from the vector part. The text list
is the CANDIDATES rows ranked highest for QUERY_TEXT, read from the text
part, of those whose rank is above 0. In each list a row's rank is 1 for
the best, 2 for the next, and so on, ties taken in DOC_ID order.

| Column | Is |
|---|---|
| DOC_ID | The row's primary key |
| SCORE | Its fused score |
| VECTOR_SCORE | 1 minus its distance, or null if it is not in the vector list |
| TEXT_SCORE | Its RANK, or null if it is not in the text list |
| VECTOR_RANK | Its rank in the vector list, or null |
| TEXT_RANK | Its rank in the text list, or null |
| CHUNK_TEXT | Its text |

It gives the TOP_N rows of the highest SCORE, ties in DOC_ID order. The
text column is the chunk: a row is one chunk of a document.

QUERY_TEXT and QUERY_VECTOR may be any value expressions, dynamic
parameters among them. The other arguments are literals.

## Fusion

RRF, reciprocal rank fusion, adds for each list that holds the row its
weight divided by RRF_K plus its rank there. A row found by both lists
gains from each:

```sql
SELECT DOC_ID, SCORE, VECTOR_RANK, TEXT_RANK FROM HYBRID_SEARCH(INDEX = HELP_H,
    QUERY_TEXT = 'refund damaged', QUERY_VECTOR = TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32'), TOP_N = 4) AS H
    ORDER BY SCORE DESC, DOC_ID;
```

```
       DOC_ID |                    SCORE |          VECTOR_RANK |            TEXT_RANK
--------------+--------------------------+----------------------+----------------------
            1 |     3.278688524590164E-2 |                    1 |                    1
            3 |      3.15136476426799E-2 |                    5 |                    2
            4 |     3.149801587301587E-2 |                    4 |                    3
            2 |    1.6129032258064516E-2 |                    2 |                 NULL
(4 rows)
```

WEIGHTED adds for each list that holds the row its weight times its
score there, normalized min-max over the list's candidates: the lowest
score of the list is 0, the highest 1. If every candidate of a list
scores alike, each is 1. A vector score is 1 minus the distance, so the
nearest row scores highest:

```sql
SELECT DOC_ID, SCORE, VECTOR_SCORE, TEXT_SCORE FROM HYBRID_SEARCH(INDEX = HELP_H,
    QUERY_TEXT = 'refund damaged', QUERY_VECTOR = TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32'), TOP_N = 4,
    FUSION = 'WEIGHTED', VECTOR_WEIGHT = 0.7, TEXT_WEIGHT = 0.3) AS H
    ORDER BY SCORE DESC, DOC_ID;
```

```
       DOC_ID |                    SCORE |             VECTOR_SCORE |               TEXT_SCORE
--------------+--------------------------+--------------------------+--------------------------
            1 |                      1E0 |                      1E0 |     1.5884793584977979E0
            2 |     6.957186069495863E-1 |     9.938837242136948E-1 |                     NULL
            5 |     4.949747528328101E-1 |     7.071067897611574E-1 |                     NULL
            4 |     6.965260190236647E-2 |     9.950371700338068E-2 |     8.984402202582297E-1
(4 rows)
```

The rows of HYBRID_SEARCH keep no order in the query around it, so
write ORDER BY there. Its DOC_ID joins it to its table:

```sql
SELECT D.ID, D.BODY FROM HYBRID_SEARCH(INDEX = HELP_H, QUERY_TEXT = 'gift',
    QUERY_VECTOR = TO_VECTOR('[0,0,1,0]', 4, 'FLOAT32'), TOP_N = 2) AS H
    JOIN HELP D ON D.ID = H.DOC_ID ORDER BY H.SCORE DESC, D.ID;
```

```
           ID | BODY
--------------+--------------------------------------------------------------
            4 | Refund of a gift card
            1 | Refund policy for damaged items
(2 rows)
```

## The plan

HYBRID_SEARCH is read as a query of the two lists joined by DOC_ID. Its
EXPLAIN shows two reads of the hybrid index, each a NearestScan: one of
RANK, of the text part, and one of the distance, of the vector part,
marked approximate with the index's metric. Under them a FullOuterJoin
puts the lists together.

## Sparse vectors

An index of ORGANIZATION INVERTED, the method `inverted`, holds sparse
vectors, a column of SPARSE layout. It serves FETCH APPROX of a DOT
distance of the column, if the column's type fixes its dimension count
and its format. The distance is the dot product negated, so the
greatest dot product comes first. A query vector of other dimensions, or
of another format, is not searched approximately: the query reads the
table, and gives the error the distance gives. A filter of the query is
asked of the candidates as the index finds them under VECTOR_INFILTER,
and of those it gives under VECTOR_POSTFILTER
([FETCH APPROX](06-queries.md#fetch-approx-and-fetch-exact-extension)).

## Errors

A search names its index, its words, its vector and its count:

```sql
SELECT * FROM HYBRID_SEARCH(INDEX = HELP_H, QUERY_TEXT = 'gift', TOP_N = 1) AS H;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): HYBRID_SEARCH names its QUERY_VECTOR (at character 80)
```

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A required argument is missing, or one is written twice |
| 2A000 | -103 | INDEX names no hybrid index, or its table's primary key is not one column |
| 2A000 | -103 | A weight is not a number |
| 2A000 | -102 | An argument is not one of those above, or FUSION is neither RRF nor WEIGHTED |
| 0A000 | -900 | A hybrid index is given a MODEL or a CHUNKER |
