# DROP VECTOR PIPELINE

[Statements](README.md)

## NAME

DROP VECTOR PIPELINE: stop keeping a table of embedded chunks

## SYNOPSIS

```
<drop vector pipeline statement> ::=
    DROP VECTOR PIPELINE <pipeline name> [ CASCADE | RESTRICT ]
```

RESTRICT is the default. Nothing depends on a pipeline, so the two are
alike.

## DESCRIPTION

DROP VECTOR PIPELINE drops a pipeline and its four triggers. Its target
keeps its rows, kept in step with the source no more.

A pipeline is dropped too, with CASCADE, with its source or target
table, or with its model. Without CASCADE such a drop is refused. DROP
VECTOR PIPELINE runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The pipeline exists, and its user owns the pipeline's schema.
2. The transaction is not READ ONLY.

## PRIVILEGES

The pipeline's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The statement is not of the form above |
| 2A000 | -103 | There is no such pipeline |
| 2A000 | -104 | Its user does not own the pipeline's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE EMBEDDING MODEL WORDS WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 4);
CREATE TABLE DOCS (ID INTEGER, BODY VARCHAR(200));
CREATE TABLE CHUNKS (DOC INTEGER, N INTEGER, PIECE VARCHAR(200), E VECTOR(4));
INSERT INTO DOCS VALUES (1, 'kept');
CREATE VECTOR PIPELINE P SOURCE DOCS (ID, BODY) EMBED USING WORDS TARGET CHUNKS (DOC, N, PIECE, E);
DROP VECTOR PIPELINE P;
DELETE FROM DOCS;
SELECT DOC, PIECE FROM CHUNKS;
```

```
OK
OK
OK
OK
OK
OK
OK
          DOC | PIECE
--------------+--------------------------------------------------------------
            1 | kept
(1 row)
```

## STANDARD

An extension: no part of SQL has vector pipelines.

## SEE ALSO

[CREATE VECTOR PIPELINE](create-vector-pipeline.md),
[DROP EMBEDDING MODEL](drop-embedding-model.md)
