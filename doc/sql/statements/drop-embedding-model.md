# DROP EMBEDDING MODEL

[Statements](README.md)

## NAME

DROP EMBEDDING MODEL: drop an embedding model

## SYNOPSIS

```
<drop embedding model statement> ::=
    DROP { EMBEDDING | EXTERNAL } MODEL <model name> [ CASCADE | RESTRICT ]
```

RESTRICT is the default.

## DESCRIPTION

DROP EMBEDDING MODEL drops a model and the grants of EXECUTE on it. A
generated column whose expression calls it, a pipeline that embeds by
it, and a view whose query calls it depend on it. With RESTRICT the
drop is refused while one does. With CASCADE they are dropped first,
the column's values with it, and the views, triggers and routines that
read such a view with the view. A pipeline's target keeps its rows.
DROP EXTERNAL MODEL is the same statement.

DROP SCHEMA with CASCADE drops the schema's models. DROP EMBEDDING
MODEL runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The model exists, and its user owns the model's schema.
2. With RESTRICT, no generated column, pipeline or view uses it.
3. The transaction is not READ ONLY.

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
CREATE EMBEDDING MODEL WORDS WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 4);
CREATE TABLE NOTES (ID INTEGER, BODY VARCHAR(100),
    E VECTOR(4) GENERATED ALWAYS AS (EMBED('WORDS', BODY)));
DROP EMBEDDING MODEL WORDS;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): generated column ANN.NOTES.E embeds by model ANN.WORDS, so it is dropped only with CASCADE (at character 21)
```

With CASCADE, the column goes with it:

```sql
DROP EMBEDDING MODEL WORDS CASCADE;
SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'NOTES' ORDER BY ORDINAL_POSITION;
```

```
OK
 COLUMN_NAME
--------------------------------------------------------------
 ID
 BODY
(2 rows)
```

## STANDARD

An extension: no part of SQL has embedding models.

## SEE ALSO

[CREATE EMBEDDING MODEL](create-embedding-model.md),
[DROP VECTOR PIPELINE](drop-vector-pipeline.md)
