# CREATE EMBEDDING MODEL

[Statements](README.md)

## NAME

CREATE EMBEDDING MODEL: define an embedding model of a plug-in

## SYNOPSIS

```
<embedding model definition> ::=
    CREATE EMBEDDING MODEL <model name> [ FROM FILE <character string literal> ]
    WITH ( <model option> [ , <model option> ]... )

<model option> ::=
    <identifier> = { <character string literal> | [ <sign> ] <unsigned numeric literal> | <identifier> }
```

EMBEDDING, MODEL and FILE are read as words, not key words, so a table
or column may still be named by one.

## DESCRIPTION

CREATE EMBEDDING MODEL defines a model in a schema: a plug-in that turns
texts into vectors, registered by the host program, and the options it
is given. EMBED and VECTOR_EMBEDDING of the model's name, a literal or a
bare name, call it, and EMBED_VERSION of it gives its version tag,
`schema.name:version`, which a pipeline's version column keeps.

TYPE names the plug-in. The database reads five options itself and
gives the plug-in the others, and FROM FILE's path as the option FILE.

| Option | Value | Default |
|---|---|---|
| TYPE | The plug-in's name | (required) |
| VERSION | A whole number, the model's version | The plug-in's own |
| BATCH_SIZE | How many texts the plug-in is asked at once | The plug-in's own |
| ON_ERROR | `FAIL`, `NULL` or `RETRY` | `FAIL` |
| MAX_RETRIES | How many times RETRY asks again | 3 |
| DIMENSIONS, FORMAT, LAYOUT | The shape of its vectors, checked against the plug-in's | The plug-in's |

ON_ERROR = 'FAIL' fails the statement the plug-in fails. 'NULL' gives
the null value for each text it refuses, asking for the others alone.
'RETRY' asks again, up to MAX_RETRIES times, then fails.

A projection of EMBED of a model, read a batch of rows at a time, gives
the plug-in the texts of many rows at once, BATCH_SIZE at a time.

The built-in plug-ins are HASH_DENSE, which takes DIMENSIONS and NGRAMS,
and HASH_SPARSE. A host registers others with
`Database::register_embedder`. A model whose plug-in is not registered,
as a database opens again before its host registers it, is defined
still, of the shape it was made of, and a call of it is refused until
the plug-in is.

INFORMATION_SCHEMA.EMBEDDING_MODELS lists the models of the schemas the
user owns and those it may call. CREATE EMBEDDING MODEL runs within the
session's transaction: ROLLBACK undoes it.

## RULES

1. The schema exists, and its user owns it.
2. No model of the schema has the name.
3. TYPE names a plug-in registered.
4. Each option is one the database or the plug-in takes, of a value it
   accepts. VERSION, BATCH_SIZE and MAX_RETRIES are whole numbers, not
   strings, and BATCH_SIZE is at least 1.
5. DIMENSIONS, FORMAT and LAYOUT, if written, are the plug-in's.
6. The transaction is not READ ONLY.

## PRIVILEGES

The schema is its user's. Its owner holds EXECUTE on the model,
grantably (see [GRANT](grant.md)).

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The statement is not of the form above |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE EMBEDDING MODEL WORDS WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 4, NGRAMS = 1, VERSION = 2);
VALUES (EMBED('WORDS', 'a short text'), EMBED_VERSION(WORDS));
SELECT MODEL_NAME, MODEL_TYPE, DIMENSIONS, ELEMENT_FORMAT, MODEL_VERSION, IS_REGISTERED
    FROM INFORMATION_SCHEMA.EMBEDDING_MODELS;
```

```
OK
                                                              |
--------------------------------------------------------------+--------------------------------------------------------------
 [0,1,0,0]                                                    | ANN.WORDS:2
(1 row)
 MODEL_NAME                                                   | MODEL_TYPE                                                   |   DIMENSIONS | ELEMENT_FORMAT                                               | MODEL_VERSION | IS_REGISTERED
--------------------------------------------------------------+--------------------------------------------------------------+--------------+--------------------------------------------------------------+---------------+--------------------------------------------------------------
 WORDS                                                        | HASH_DENSE                                                   |            4 | FLOAT32                                                      |             2 | YES
(1 row)
```

A model's shape is its plug-in's:

```sql
CREATE EMBEDDING MODEL SPARSE WITH (TYPE = 'HASH_SPARSE', LAYOUT = DENSE);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): VEC-0031 the model's vectors are VECTOR(1048576, FLOAT32, SPARSE), not of LAYOUT DENSE (at character 23)
```

## STANDARD

An extension: no part of SQL has embedding models.

## SEE ALSO

[ALTER EMBEDDING MODEL](alter-embedding-model.md),
[DROP EMBEDDING MODEL](drop-embedding-model.md),
[CREATE VECTOR PIPELINE](create-vector-pipeline.md),
[GRANT](grant.md)
