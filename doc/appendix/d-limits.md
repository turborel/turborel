# Appendix D: Limits

[Contents](README.md)

SQL sets few limits of its own. A statement past one of these is valid
SQL Turborel does not process: it is refused with SQLCODE -900,
SQLSTATE 0A000.

## Lengths and precisions

| What | Limit |
|---|---|
| An identifier | 128 characters |
| Levels of a partitioning tree below its root *(extension)* | 16 |
| Partitions of a tree with a global index, by their ids *(extension)* | 16,777,216 |
| Rows of a partition with a global index, by their ids *(extension)* | 1,099,511,627,776 |
| CHARACTER and CHARACTER VARYING | 32,767 characters |
| BINARY and BINARY VARYING | 32,767 octets |
| CHARACTER LARGE OBJECT and BINARY LARGE OBJECT | 4G, 4,294,967,296 characters or octets |
| A LIKE pattern or escape, or a regular expression's pattern, flag or replacement, of a large object | 262,144 octets, a chunk; a LIKE match value under UNICODE or a tailoring too |
| NUMERIC, DECIMAL, an exact literal and an exact result | 38 digits |
| FLOAT | 53 binary digits; an approximate literal within the range of DOUBLE PRECISION |
| DECFLOAT | 34 digits; an exponent of its leading digit from -6143 to 6144 |
| A datetime's fraction of a second | 6 digits |
| An interval's leading field | 9 digits; its fraction of a second 6 |
| An unsigned integer, as a length | 4,294,967,295 |
| An array's maximum cardinality | 65,535 elements, also the maximum of an array type written without one |
| A multiset's elements | 65,535; past it, SQLSTATE 2200Q |
| An XML value | Elements nested 512 deep; 16 MiB of text and 1,048,576 nodes and namespace bindings in one tree; 16 MiB more of entities' text in one parse; 64 MiB of text XMLPARSE reads; a stored form of 64 MiB |
| An XQuery query | Expressions nested 64 deep; operations and function calls nested 256 deep as it is evaluated, expressions 256 deep as it is statically typed, a static type 256 deep, 4,096 item types an axis's closure meets; 1,048,576 items in one sequence; 64 MiB of nodes one evaluation constructs |
| A number in JSON text or an SQL/JSON path | an exact one within DECFLOAT's range, an approximate one within DOUBLE PRECISION's |
| The precision of an SQL/JSON item method | decimal() 38 digits; time(), time_tz(), timestamp() and timestamp_tz() 6, and a string datetime() reads, or its template's FF*n*, 6 digits of a second's fraction |
| A FULLTEXT value or a full-text search pattern | 32,767 characters; past it, SQLSTATE 22001 |
| A full-text language's name | 32 characters |
| The array a full-text tokenizing method gives | 65,535 elements; past it, SQLSTATE 2202F |
| The relations of the thesauri a statement reads | 65,535 |
| A VECTOR's dimension count | 1 to 65,535; of a sparse vector 2,147,483,647; of BINARY 8 to 65,528, a multiple of 8 |
| A vector's text in a character large object | 16,777,216 octets; past it, SQLSTATE 22003 |
| The text EMBED and the chunking functions read of a character large object | 16,777,216 octets; past it, SQLSTATE 22000 |

## Nesting

A statement is at most 64 levels deep as it is read: about 30 nested
parentheses, 60 `AND`, `OR`, `+` or `UNION` in a row, or 60 tables in a
FROM clause. A deeper one is refused rather than run out of stack.

Triggers run within triggers at most 16 deep: a trigger whose action
runs a 17th fails its statement with SQLSTATE 09000, SQLCODE -900.

A full-text search pattern nests at most 100 deep, and a deeper one is
SQLSTATE 2FF01.

Array, multiset and row types nest at most 16 deep, an array of rows of
arrays counting three. A deeper type is refused as it is read.

A regular expression compiles to at most 8,192 instructions, about as
many characters, each counted repeat as many times as its count, so
`a{9000}` is refused. Its groups nest at most 64 deep. A search of a
pattern with a back-reference goes back to try each way and stops past
10,000,000 steps; one without any takes time linear in the string, with
no limit. Each is SQLCODE -900.

JSON text nests arrays and objects at most 128 deep, and an SQL/JSON
path expression is at most 64 deep, of parentheses, filters, subscripts
and signs, or of accessors and operators in a row. A deeper one is
SQLCODE -900, which ON ERROR does not replace. JSON text, or a PASSING
argument, in a large object is read at most 262,144 octets long, a chunk.

A JSON object JSON_OBJECT or JSON_OBJECTAGG makes has at most 1,048,576
members, and an array JSON_ARRAY or JSON_ARRAYAGG makes at most
1,048,576 elements, those left out ABSENT ON NULL not counted; one more
is SQLSTATE 2203E or 2203D. JSON_ARRAYAGG holds the rows of a group
until it orders them, so it holds no more than that.

## Grouping

A GROUP BY stands for at most 4,096 grouping sets: a CUBE of 12 columns
or sets, or ROLLUPs and GROUPING SETS whose sets, multiplied, come to no
more. GROUPING names at most 59 columns, since its value, up to 2 to
their number less 1, has at most the 18 digits of its type.

## Sizes

| What | Limit |
|---|---|
| An index's key | of the default 8 KiB page, 1,983 bytes in a B-link tree, 8,112 in a hash index; of a 4 KiB page, 959 in a B-link tree; of a 64 KiB page, 16,319 in a B-link tree, 65,456 in a hash index |
| A word a full-text index keeps | its first 256 characters, and of those its first 255 bytes in UTF-8; a longer word is found by that prefix and checked against the row |
| A document a full-text index reads | none: it is read a word at a time |
| A GEOMETRY value | 16 MiB of WKT or WKB, 100,000 components, rings and coordinates in all, and 16 levels of nesting, the value's own included; more is refused with SQLCODE -900 |
| A GEOMETRY value's WKT or WKB in SQL | 16 MiB: ST_ASTEXT returns a CHARACTER LARGE OBJECT and ST_ASBINARY a BINARY LARGE OBJECT, the constructors take one, each up to the limit above ([Spatial Data](../sql/10-spatial.md)) |
| A row | none: its long values are kept out of line |
| A message to the server | SERVER.MAX_MESSAGE_BYTES, 16 MiB by default |
| Connections at once | SERVER.MAX_CONNECTIONS, 100 by default |
| Prepared statements of a connection | SERVER.MAX_STATEMENTS, 1,024 by default |
| Savepoints of a transaction | 1,024; past it, SAVEPOINT fails with SQLSTATE 3B002, SQLCODE -917 |

An index's key is compared whole in its page, so it is limited where a
row is not; STORAGE.PAGE_BYTES, chosen as a database is made, raises the
limit. A column of a collation is keyed by its sort key, about 16 bytes for
each character under UNICODE and half as many again under a language's
collation, so its index holds shorter text.
