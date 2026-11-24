# CREATE VECTOR PIPELINE

[Statements](README.md)

## NAME

CREATE VECTOR PIPELINE: keep a table of embedded chunks in step with a table of texts *(extension)*

## SYNOPSIS

```
<vector pipeline definition> ::=
    CREATE VECTOR PIPELINE <pipeline name>
    SOURCE <table name> ( <key column> [ , <key column> ]... , <text column> )
    [ CHUNK BY <chunk unit> MAX <unsigned integer> [ OVERLAP <unsigned integer> ]
        [ SPLIT_ON <character string literal> ] [ NORMALIZE WHITESPACE ] ]
    EMBED USING <model name>
    TARGET <table name> ( <key column> [ , <key column> ]... ,
        <chunk id column> , <chunk text column> , <embedding column> [ , <version column> ] )
    [ MODE { SYNC | ASYNC } ]

<chunk unit> ::= CHARS | WORDS | SENTENCES | PARAGRAPHS | TOKENS | LINES
```

VECTOR, PIPELINE, SOURCE, CHUNK, EMBED, USING, TARGET and MODE are read
as words, not key words.

## DESCRIPTION

A pipeline keeps its target, a table that exists already, in step with
its source. Each text of the source is cut into chunks as CHUNK_TEXT
cuts it (by words, at most 256 of them, if CHUNK BY is not written), and
each chunk is a row of the target: the source row's keys, the chunk's
number from 1, its text, its embedding by the model, and, if a fifth
column is named after the keys, the model's version tag.

As the pipeline is made, the target's rows are deleted and the chunks
of every source row inserted. Four triggers on the source, made with it
and named after it, keep the target after: an insert adds the new row's
chunks, a delete removes the old row's, and an update of the text or a
key does both.

MODE SYNC, the default, embeds each chunk as it is inserted. MODE ASYNC
inserts it with its embedding and version null, and the queue after
commit fills them. A job runs after each commit that wrote, and as the
database opens, and reads the tables written since its last round. It
embeds the chunks waiting in slices of at most 4096 distinct texts,
each slice's texts given the model BATCH_SIZE at a time, in a
transaction of the target's schema's owner. What waits is in the rows
themselves, so it survives a crash. A slice that fails leaves its rows
for the next round after their table is written again.

A statement whose text begins with the hint `/*+ VECTOR_WAIT_FRESH */`,
run outside a transaction, runs a round of the queue first, so what it
reads is fresh. A version column whose tag is not the model's has its
chunk embedded again by the queue, in either mode, after ALTER
EMBEDDING MODEL gives the model a new version.

INFORMATION_SCHEMA.VECTOR_PIPELINES lists the pipelines of the schemas
the user owns, and INFORMATION_SCHEMA.EMBEDDING_QUEUE each column the
queue fills. It shows the distinct texts the column's last round found
waiting, the rows the queue has embedded, its failed rounds and its last
error. CREATE VECTOR PIPELINE runs
within the session's transaction: ROLLBACK undoes it.

## RULES

1. The source and target are base tables of schemas its user owns, and
   the model is one it may call.
2. No pipeline of the schema has the name, and no trigger the names of
   its triggers.
3. The source has each column named, and its text is of a character
   type. The target has each column named, as many keys as the source,
   then three or four columns more.
4. The chunking is one CHUNK_TEXT accepts.
5. The transaction is not READ ONLY.

## PRIVILEGES

The schemas are its user's, and it holds EXECUTE on the model.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | The statement is not of the form above |
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | Its user does not own a schema, or may not call the model |
| 22000 | -802 | CHUNK_TEXT refuses the chunking |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE EMBEDDING MODEL WORDS WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 4);
CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(200));
CREATE TABLE CHUNKS (DOC INTEGER, N INTEGER, PIECE VARCHAR(200), E VECTOR(4), V VARCHAR(100));
INSERT INTO DOCS VALUES (1, 'one two three four five');
CREATE VECTOR PIPELINE DOCS_CHUNKS SOURCE DOCS (ID, BODY) CHUNK BY WORDS MAX 2
    EMBED USING WORDS TARGET CHUNKS (DOC, N, PIECE, E, V);
INSERT INTO DOCS VALUES (2, 'six');
SELECT DOC, N, PIECE, E IS NULL, V FROM CHUNKS ORDER BY DOC, N;
```

```
OK
OK
OK
OK
OK
OK
          DOC |            N | PIECE                                                        |       | V
--------------+--------------+--------------------------------------------------------------+-------+--------------------------------------------------------------
            1 |            1 | one two                                                      | FALSE | ANN.WORDS:1
            1 |            2 | three four                                                   | FALSE | ANN.WORDS:1
            1 |            3 | five                                                         | FALSE | ANN.WORDS:1
            2 |            1 | six                                                          | FALSE | ANN.WORDS:1
(4 rows)
```

Of MODE ASYNC, the chunks wait for the queue after commit:

```sql
CREATE TABLE LATER (DOC INTEGER, N INTEGER, PIECE VARCHAR(200), E VECTOR(4));
CREATE VECTOR PIPELINE DOCS_LATER SOURCE DOCS (ID, BODY) CHUNK BY WORDS MAX 3
    EMBED USING WORDS TARGET LATER (DOC, N, PIECE, E) MODE ASYNC;
SELECT DOC, N, PIECE, E IS NULL FROM LATER ORDER BY DOC, N;
COMMIT;
/*+ VECTOR_WAIT_FRESH */ SELECT DOC, N, E IS NULL FROM LATER ORDER BY DOC, N;
SELECT TABLE_NAME, COLUMN_NAME, MODEL_VERSION, EMBEDDED, FAILURES
    FROM INFORMATION_SCHEMA.EMBEDDING_QUEUE ORDER BY TABLE_NAME;
```

```
OK
OK
          DOC |            N | PIECE                                                        |
--------------+--------------+--------------------------------------------------------------+-------
            1 |            1 | one two three                                                | TRUE
            1 |            2 | four five                                                    | TRUE
            2 |            1 | six                                                          | TRUE
(3 rows)
OK
          DOC |            N |
--------------+--------------+-------
            1 |            1 | FALSE
            1 |            2 | FALSE
            2 |            1 | FALSE
(3 rows)
 TABLE_NAME                                                   | COLUMN_NAME                                                  | MODEL_VERSION                                                |     EMBEDDED |     FAILURES
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------+--------------
 CHUNKS                                                       | E                                                            | ANN.WORDS:1                                                  |            0 |            0
 LATER                                                        | E                                                            | ANN.WORDS:1                                                  |            3 |            0
(2 rows)
```

## STANDARD

An extension: no part of SQL has vector pipelines.

## SEE ALSO

[DROP VECTOR PIPELINE](drop-vector-pipeline.md),
[CREATE EMBEDDING MODEL](create-embedding-model.md),
[CREATE TRIGGER](create-trigger.md)
