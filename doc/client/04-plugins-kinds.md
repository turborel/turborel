# Kinds of Plug-in

[Contents](README.md)

Besides functions, aggregates, table functions, procedures, metrics,
embedding models, authenticators, backup targets and partitioning
strategies ([Plug-ins](04-plugins.md)), an extension may bring a
full-text ranking, a vector quantizer, a collation, a system table, an
XQuery function, a data type, a storage engine, an index method, the
database's credentials and a sink of its events. Each is the in-process
extension point of its kind, served by the program over protocol 1.

Every call crosses a pipe and costs a round trip, so the database asks
a batch at a time and never asks inside a comparison, a page read or a
commit's critical section. What it needs there it works out alone, from
what one call gave it before: a codebook, a sort key, a type's key. Each
section below gives a kind's declaration, its operations' payloads, what
it costs, and an example run with the example plug-in, whose manifest
declares every object shown.

```sql
CREATE EXTENSION EXAMPLE;
```

```
OK
```

## Rankings

```
RANKING <identifier>
```

A ranking scores full-text matches for RANK and a top-k search, named by
an index's FULLTEXT.RANKING. Kind 10 has two operations.

```text
corpus   := documents u64 | average_length f64 | k1 f64 | b f64
term     := documents u64 | most u32
WEIGHTS  corpus | count u32 | (term | frequency u32 | length u32)...   -> count u32 | f64...
BOUNDS   corpus | count u32 | term...                                  -> count u32 | f64...
```

WEIGHTS gives the weight of many (term, document) pairs in one call, and
BOUNDS each term's greatest weight, so a top-k search asks once a block
of candidates. A weight that is not a number fails the statement.

```sql
CREATE TABLE DOCS (ID INTEGER NOT NULL UNIQUE, BODY VARCHAR(200));
INSERT INTO DOCS VALUES (1, 'a dog'), (2, 'a cat'), (3, 'dog eats dog dog');
COMMIT;
CREATE INDEX DOCS_BODY ON DOCS (BODY) USING FULLTEXT WITH (FULLTEXT.RANKING = COUNTED);
SELECT ID FROM DOCS WHERE CONTAINS(BODY, '"dog"') = 1 ORDER BY RANK(BODY, '"dog"') DESC;
```

```
OK
OK
OK
OK
           ID
--------------
            3
            1
(2 rows)
```

## Quantizers

```
QUANTIZER <identifier>
```

A quantizer compresses the vectors a vector index searches by, named by
the index's QUANTIZER option. Kind 11 has one operation.

```text
TRAIN  dims u32 | code_bytes u32 | seed u64 | count u32 | (f32s)...
       -> k u32 | count u32 | width u32... | (f32s)... one a subspace, k centroids of its width
```

TRAIN turns a sample into a product codebook, `k` centroids in each of
the subspaces its widths split the dimensions into. The database keeps
the codebook and encodes, decodes and measures by it alone, so a search
makes no call. The index trains when it is built, or when maintenance
finds TRAIN_AT rows indexed.

```sql
CREATE TABLE V (K INTEGER PRIMARY KEY, E VECTOR(2));
INSERT INTO V VALUES (1, '[1, 1]'), (2, '[2, 5]'), (3, '[8, 1]'), (4, '[9, 9]');
COMMIT;
CREATE INDEX V_E ON V (E) USING DISKANN WITH (DISKANN.METRIC = EUCLIDEAN, DISKANN.QUANTIZER = FIRSTS, DISKANN.PQ_BYTES = 1);
SELECT K FROM V ORDER BY VECTOR_DISTANCE(E, '[8, 2]', EUCLIDEAN) FETCH APPROX FIRST 1 ROWS ONLY;
```

```
OK
OK
OK
OK
            K
--------------
            3
(1 row)
```

## Collations

```
COLLATION <identifier> VERSION <character string literal>
```

A collation orders and compares character strings, named by COLLATE as
a built-in one is. Kind 12 has one operation.

```text
KEYS  pad u8 (0 PAD SPACE, 1 NO PAD) | count u32 | str...   -> count u32 | bytes...
```

KEYS gives the sort keys of many strings at once, and the database
compares keys as bytes. A sort, a grouping or an index of a column of
the collation asks once a batch of values, and no comparison crosses
the pipe. Its VERSION is checked as a built-in collation's is, so an
index made by an older version is refused until it is rebuilt. LIKE,
TRIM, an XQuery collation and a key made character by character need
the characters themselves, which a sort key does not give, so a plug-in
collation is refused there with SQLCODE -103.

```sql
CREATE TABLE PEOPLE (N VARCHAR(20) COLLATE FOLDED);
INSERT INTO PEOPLE VALUES ('b'), ('A'), ('a'), ('B');
SELECT N FROM PEOPLE ORDER BY N, N COLLATE UCS_BASIC;
SELECT N FROM PEOPLE WHERE N LIKE 'a%';
```

```
OK
OK
 N
----------------------
 A
 a
 B
 b
(4 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): LIKE compares no strings by the plug-in collation FOLDED, whose keys are calls made a batch at a time
```

## System tables

```
SYSTEM TABLE <identifier> ( <column name> <data type>, ... )
```

A system table is read in INFORMATION_SCHEMA as the built-in ones are.
Kind 13 has one operation.

```text
ROWS  user str | owner u8 | from u64 | max u32   -> rows | more u8
```

ROWS is asked as a statement reads the table, with the statement's user
and whether it is the database's owner, and answers a page of at most
`max` rows, BATCH of them (1,024), after the `from` given before, and
whether more follow. At most 1,000,000 rows are taken, each value
checked against its column.

```sql
SELECT COUNT(*), MAX(N) FROM INFORMATION_SCHEMA.EXAMPLE_FACTS WHERE NAME = 'ROW';
```

```
                      |
----------------------+--------------
                 2500 |         2500
(1 row)
```

## XQuery functions

```
XQUERY FUNCTION <character string literal> ( <data type>, ... ) RETURNS <data type>
```

An XQuery function is called in XMLQUERY by its expanded name, written
in the manifest as its namespace's URI in braces before its local name.
Its parameters and result are atomic, of the types that map to
xs:string, xs:integer, xs:decimal, xs:double and xs:boolean, each
optional. Kind 14 has one operation, CALL, of one row of arguments to
one value, as a function's is, so each call in a query is a round trip.

```sql
VALUES (XMLCAST(XMLQUERY('declare namespace f = "http://example.com/fn"; f:twice(21)' RETURNING SEQUENCE BY REF EMPTY ON EMPTY) AS INTEGER));
```

```

--------------
           42
(1 row)
```

## Types

```
TYPE <identifier> [ ORDERED ]
```

A type is a column's type, named as a built-in one is. Kind 15 has two
operations.

```text
INPUT    count u32 | str...     -> count u32 | made...
RECEIVE  count u32 | bytes...   -> count u32 | made...
made:    key bytes | binary bytes | text str
```

INPUT makes values of their text forms, a cast from a character string,
and RECEIVE of their binary forms, a cast from a binary string, a batch
at a time. Each value is the three byte strings the program made, which
the database stores: a key compared as bytes, the binary form and the
text form. It compares, groups, hashes and indexes by the key, and casts
to character by the text form and to binary by the binary form, so
nothing after a value is made calls the program. A type declared ORDERED
may be ordered and range-indexed, one not only compared for equality.
It has no arithmetic, no cast to or from another type, no type modifier
and no collation.

```sql
CREATE TABLE P (K INTEGER, X PAIR);
INSERT INTO P VALUES (1, CAST('(2,1)' AS PAIR)), (2, CAST('(1,5)' AS PAIR)), (3, CAST('(-1, 0)' AS PAIR));
SELECT K, CAST(X AS VARCHAR(20)) FROM P ORDER BY X;
```

```
OK
OK
            K |
--------------+----------------------
            3 | (-1,0)
            2 | (1,5)
            1 | (2,1)
(3 rows)
```

## Storage engines

```
STORAGE ENGINE <identifier> [ BATCH <unsigned integer> ]
```

A storage engine keeps the rows of each table made USING it. Kind 16
has seven operations, each of a table's locator, which CREATE gives.

```text
CREATE      table str | columns: count u32 | name str...      -> locator bytes
OPEN        locator bytes                                     -> found u8 | next u64 | rows u64 | durable u64
SCAN        locator bytes | after u64 | max u32               -> count u32 | (id u64 | row)... | more u8
GET         locator bytes | count u32 | id u64...             -> count u32 | (found u8 | row if found)...
APPLY       locator bytes | count u32 | (id u64 | change)...  -> count u32 | before...
CHECKPOINT  locator bytes | through u64                       -> (nothing)
DROP        locator bytes                                     -> (nothing)
change, before: u8 0 (no row) | u8 1 and row
```

The database keeps the transactions. Its table locks isolate them, a
writer holding its table to its end, so the program never sees two at
once. A transaction's writes stay in the database until it commits.
Then the rows it changes are read (GET) and its writes applied (APPLY),
BATCH rows a call (1,024 if not written), each call all or none. A call
that fails puts every row back and fails the commit. A read merges the
program's rows, a page of BATCH rows a SCAN in ascending order of id, or
a GET by id, with the transaction's own writes.

The program keeps the position it is durable through, given at a
CHECKPOINT, and OPEN gives it back. Recovery applies the log's changes
from there, each an APPLY of one row, so APPLY must be idempotent. A
process of the extension that stops while a transaction writes a table
fails that transaction at commit with SQLSTATE 40001, to run again, so a
program restarted never commits half of one.

What a row costs is its share of a call. Measured on a debug build of
20,000 rows with BATCH 100 and a program that makes each APPLY durable,
a commit costs about 10 microseconds a row more than the default
engine's, a scan about 2 microseconds a row more, and a read by id one
round trip, about a quarter of a millisecond.

```sql
CREATE TABLE T (K INTEGER, V VARCHAR(20)) USING FILES;
INSERT INTO T VALUES (1, 'one'), (2, 'two'), (3, 'three');
COMMIT;
SELECT K, V FROM T ORDER BY K;
```

```
OK
OK
OK
            K | V
--------------+----------------------
            1 | one
            2 | two
            3 | three
(3 rows)
```

## Index methods

```
INDEX METHOD <identifier> SUPPORTS ( EQUALITY | RANGE | NEAREST, ... ) [ BATCH <unsigned integer> ]
```

An index method keeps each index made USING it, and serves the searches
its SUPPORTS names: EQUALITY keys equal to one given, RANGE comparisons,
prefixes, ranges and order, NEAREST the nearest entries by a vector
query's metric, an approximate search. Kind 17 has eight operations.

```text
CREATE      columns: count u32 | type str...                  -> locator bytes
OPEN        locator bytes                                     -> found u8 | clean u8
WRITE       locator bytes | count u32 | (insert u8 | id u64 | key)...   -> (nothing)
SEARCH      locator bytes | probe | descending u8 | cursor bytes | max u32
                                                              -> entries | cursor bytes
ROWS        locator bytes | key                               -> count u32 | id u64...
NEAREST     locator bytes | column u32 | value | metric str | first u32 | descending u8
                                                              -> count u32 | (distance f64 | id u64 | key)...
CHECKPOINT  locator bytes                                     -> (nothing)
DROP        locator bytes                                     -> (nothing)
key:        count u32 | value...
entries:    count u32 | (id u64 | key)...
probe:      u8 0 every entry
            u8 1 | op u8 | value                       the first column compared
            u8 2 | equal: count u32 | value... | u8 0, or u8 1 | op u8 | value
            u8 3 | equal: count u32 | value... | low op u8 | value | high op u8 | value
op:         0 =, 1 <>, 2 <, 3 >, 4 <=, 5 >=
```

Inserts and deletes, a rollback's included, are given BATCH at a time
(WRITE), before any search and at a checkpoint. SEARCH gives a page of
entries in key order, which the database checks, and the cursor to go
on from. The index is a list of candidates. Each row is read again as
the reader's transaction sees it and kept only if it still has the key,
so the entries of a transaction not committed are no one else's answers.
OPEN says whether the program has the index and whether it is clean, as
it is after a CHECKPOINT with no write since. One not found, or not
clean as a crash leaves it, is rebuilt from its table as the database
opens.

```sql
COMMIT;
CREATE INDEX T_K ON T (K) USING SORTED;
SELECT V FROM T WHERE K BETWEEN 2 AND 3 ORDER BY K DESC;
```

```
OK
OK
 V
----------------------
 three
 two
(2 rows)
```

## Credentials and sinks of events

```
CREDENTIALS <identifier>
DIAGNOSTICS <identifier> [ BATCH <unsigned integer> ]
```

Credentials make each user's verifier, in place of those the host
gives, as `CREATE USER ... PASSWORD` and `ALTER USER` run. At most one
extension brings them. A sink of events is told each event the
database emits, as a sink the host gives is. The events are queued, at
most 4,096, the rest counted, and sent BATCH at a time (256 if not
written) by a thread of their own, so emitting never waits for a pipe.

```text
CREDENTIALS 1 verifier  user str | password str               -> bytes
DIAGNOSTICS 1 events    count u32 | (level str | kind str | text str)... | dropped u64  -> (nothing)
```

## The points a plug-in may not serve

Ten extension points that a program embedding the database may fill
have no kind of plug-in, as a call over a pipe cannot be made where
they are used. An encoding transforms each page as it is read and
written, and a hash function checksums it, both before the catalog that
holds the extensions is open. The clock is read in a commit's critical
section, and a keyring's keys must never leave the process. The log and
its storage are written by every commit and read first by recovery, and
catalog storage holds the extensions themselves. Spill relieves the
memory budget when memory is short, a served connection belongs to the
server beneath every database, and the cryptography that checks a
package's signature cannot come from the package it checks.
