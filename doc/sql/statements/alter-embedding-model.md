# ALTER EMBEDDING MODEL

[Statements](README.md)

## NAME

ALTER EMBEDDING MODEL: change an embedding model's options *(extension)*

## SYNOPSIS

```
<alter embedding model statement> ::=
    ALTER { EMBEDDING | EXTERNAL } MODEL <model name>
    WITH ( <model option> [ , <model option> ]... )
```

## DESCRIPTION

ALTER EMBEDDING MODEL merges its options into the model's: each one
written replaces the model's of its name, and the others stay. Its
plug-in is given the options again. ALTER EXTERNAL MODEL is the same
statement, its LOCATION given as ENDPOINT (see [CREATE EMBEDDING
MODEL](create-embedding-model.md)).

A new VERSION, or a plug-in that reports a new version of its own,
gives the model a new version tag. The chunks of a pipeline whose
target has a version column, kept with an older tag, are embedded again
by the queue after commit (see [CREATE VECTOR
PIPELINE](create-vector-pipeline.md)).

The model's vectors keep their shape: a change of TYPE, DIMENSIONS,
FORMAT or LAYOUT that gives another is refused. ALTER EMBEDDING MODEL
runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The model exists, and its user owns the model's schema.
2. Each option is one the database or the plug-in takes, of a value it
   accepts, as for [CREATE EMBEDDING MODEL](create-embedding-model.md).
3. The model's vectors keep their shape.
4. The transaction is not READ ONLY.

## PRIVILEGES

The model's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The statement is not of the form above |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the model's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE EMBEDDING MODEL WORDS WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 8);
ALTER EMBEDDING MODEL WORDS WITH (VERSION = 3, ON_ERROR = 'NULL');
VALUES (EMBED_VERSION(WORDS));
```

```
OK
OK

--------------------------------------------------------------
 ANN.WORDS:3
(1 row)
```

Its vectors keep their shape:

```sql
ALTER EMBEDDING MODEL WORDS WITH (DIMENSIONS = 16);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): VEC-0031 ALTER EMBEDDING MODEL keeps the shape of ANN.WORDS's vectors (at character 22)
```

## STANDARD

An extension: no part of SQL has embedding models.

## SEE ALSO

[CREATE EMBEDDING MODEL](create-embedding-model.md),
[DROP EMBEDDING MODEL](drop-embedding-model.md)
