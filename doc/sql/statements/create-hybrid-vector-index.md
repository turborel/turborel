# CREATE HYBRID VECTOR INDEX

[Statements](README.md)

## NAME

CREATE HYBRID VECTOR INDEX: make an index of a text column and a vector
column together, or of a text it embeds, for HYBRID_SEARCH *(extension)*

## SYNOPSIS

```
<create hybrid index statement> ::=
    CREATE HYBRID VECTOR INDEX <index name> ON <table name>
        ( <text column name> [ , <vector column name> ] )
    [ PARAMETERS ( <hybrid parameter> [ { , <hybrid parameter> }... ] )
    | WITH ( <table option> [ { , <table option> }... ] ) ]

<hybrid parameter> ::=
      VECTOR_IDX <character string literal>
    | TEXT_IDX <character string literal>
    | MODEL <model name>
    | CHUNKER <chunk specification>
    | MODE { SYNC | ASYNC }
```

## DESCRIPTION

CREATE HYBRID VECTOR INDEX makes an index of the method `hybrid`: a
full-text index of the text column and a vector index of the vector
column, which [HYBRID_SEARCH](../15-hybrid-search.md#hybrid_search)
reads together. It is a [CREATE INDEX](create-index.md) of that method
and its options, and is listed by INFORMATION_SCHEMA.INDEXES and
INDEX_OPTIONS as any index is.

VECTOR_IDX's string is a list of parameters as
[CREATE VECTOR INDEX](create-vector-index.md) takes them. TYPE names the
vector part's method, HNSW by default, or IVF, DISKANN or INVERTED, and
sets the option HYBRID.VECTOR_METHOD. DISTANCE sets the method's METRIC,
and each other parameter the method's option of its name. TEXT_IDX's
string is a list of words and values, each an option of the group
FULLTEXT. In the WITH form each option is written as CREATE INDEX
writes it.

A row whose text is null is held by the vector part alone, and one
whose embedding is null by the text part alone.

Of a MODEL, the index names its text column alone and embeds the text
itself (HYBRID.MODEL). CHUNKER cuts the text into chunks as CHUNK_TEXT
reads a chunking (HYBRID.CHUNKER), and MODE says whether a chunk is
embedded as it is written or after commit (HYBRID.MODE). The chunks are
kept in a table named after the index with `_CHUNKS`, by a vector
pipeline named after it with `_PIPELINE`, and the index is of that
table's CHUNK_TEXT and EMBEDDING. Both are made and filled, and
committed, before the index is made, and dropped with it.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The table is a base table, not a temporary one, and its user owns
   the table's schema.
3. The first column is a character string, the second a VECTOR column
   of a type the vector part's method indexes.
4. No index has the name.
5. TYPE names a vector method, and each option is one a part's method
   declares, of a value it accepts.
6. Of a MODEL, one column is named, the table's primary key is one
   column, the model is one the user may call, and no table or pipeline
   has the name the index gives its own. Of none, two columns are named,
   and no CHUNKER or MODE is given.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the table's schema |
| 22000 | -301 | CHUNKER is no chunking CHUNK_TEXT reads |
| 0A000 | -900 | A column is not of a type its part indexes, or one column is named and no MODEL |

## EXAMPLES

Of a table HELP (ID INTEGER PRIMARY KEY, BODY VARCHAR(200), EMB
VECTOR(4)):

```sql
CREATE HYBRID VECTOR INDEX HELP_IVF ON HELP (BODY, EMB)
    WITH (HYBRID.VECTOR_METHOD = IVF, IVF.METRIC = EUCLIDEAN, FULLTEXT.LANGUAGE = NONE);
SELECT OPTION_NAME, OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS
    WHERE INDEX_NAME = 'HELP_IVF' AND VALUE_SOURCE <> 'DEFAULT' ORDER BY OPTION_NAME;
```

```
OK
 OPTION_NAME                                                  | OPTION_VALUE
--------------------------------------------------------------+--------------------------------------------------------------
 FULLTEXT.LANGUAGE                                            | NONE
 HYBRID.VECTOR_METHOD                                         | IVF
 IVF.METRIC                                                   | EUCLIDEAN
(3 rows)
```

An index of a MODEL embeds one text column:

```sql
COMMIT;
CREATE HYBRID VECTOR INDEX HELP_M ON HELP (BODY, EMB) PARAMETERS (MODEL WORDS);
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): a hybrid index of a MODEL indexes its text column alone, and embeds it
```

## STANDARD

An extension: neither SQL-86 nor SQL-92 has indexes. The statement is
the form of §11.1 of Turborel's vector design document, with MODE added.

## SEE ALSO

[Hybrid Search](../15-hybrid-search.md),
[CREATE VECTOR INDEX](create-vector-index.md),
[CREATE INDEX](create-index.md), [DROP INDEX](drop-index.md),
[CREATE VECTOR PIPELINE](create-vector-pipeline.md)
