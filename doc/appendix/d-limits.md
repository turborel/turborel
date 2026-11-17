# Appendix D: Limits

[Contents](README.md)

SQL sets few limits of its own. A statement past one of these is valid
SQL Turborel does not process: it is refused with SQLCODE -900,
SQLSTATE 0A000.

## Lengths and precisions

| What | Limit |
|---|---|
| An identifier | 128 characters |
| CHARACTER and CHARACTER VARYING | 32,767 characters |
| BINARY and BINARY VARYING | 32,767 octets |
| CHARACTER LARGE OBJECT and BINARY LARGE OBJECT | 4G, 4,294,967,296 characters or octets |
| A LIKE pattern or escape, or a regular expression's pattern, flag or replacement, of a large object | 262,144 octets, a chunk; a LIKE match value under UNICODE or a tailoring too |
| NUMERIC, DECIMAL and an exact literal | 18 digits |
| FLOAT | 53 binary digits; an approximate literal within the range of DOUBLE PRECISION |
| DECFLOAT | 34 digits; an exponent of its leading digit from -6143 to 6144 |
| A datetime's fraction of a second | 6 digits |
| An interval's leading field | 9 digits; its fraction of a second 6 |
| An unsigned integer, as a length | 4,294,967,295 |
| An array's maximum cardinality | 65,535 elements, also the maximum of an array type written without one |
| A multiset's elements | 65,535; past it, SQLSTATE 2200Q |
| A number in JSON text or an SQL/JSON path | an exact one within DECFLOAT's range, an approximate one within DOUBLE PRECISION's |
| The precision of an SQL/JSON item method | decimal() 18 digits; time(), time_tz(), timestamp() and timestamp_tz() 6, and a string datetime() reads 6 digits of a second's fraction |

## Nesting

A statement is at most 64 levels deep as it is read: about 30 nested
parentheses, 60 `AND`, `OR`, `+` or `UNION` in a row, or 60 tables in a
FROM clause. A deeper one is refused rather than run out of stack.

Triggers run within triggers at most 16 deep: a trigger whose action
runs a 17th fails its statement with SQLSTATE 09000, SQLCODE -900.

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

## Grouping

A GROUP BY stands for at most 4,096 grouping sets: a CUBE of 12 columns
or sets, or ROLLUPs and GROUPING SETS whose sets, multiplied, come to no
more. GROUPING names at most 59 columns, since its value, up to 2 to
their number less 1, has at most the 18 digits of its type.

## Sizes

The GEOMETRY readers accept at most 16 MiB of WKT or WKB,
100,000 geometry components, rings and coordinates in total, and 16
nested geometry levels including the root. SQL text/binary interchange
still uses the 32,767-character/octet string limits above; the Rust
geometry codecs support the larger inputs. A SQL geometry observer whose
text or binary result exceeds its limit fails with SQLCODE -900.

| What | Limit |
|---|---|
| An index's key | of the default 8 KiB page, 2,007 bytes in a B-link tree, 8,116 in a hash index; of a 4 KiB page, 983 in a B-link tree; of a 64 KiB page, 16,343 in a B-link tree, 65,460 in a hash index |
| A row | none: its long values are kept out of line |
| A message to the server | SERVER.MAX_MESSAGE_BYTES, 16 MiB by default |
| Connections at once | SERVER.MAX_CONNECTIONS, 100 by default |
| Prepared statements of a connection | SERVER.MAX_STATEMENTS, 1,024 by default |

An index's key is compared whole in its page, so it is limited where a
row is not; STORAGE.PAGE_BYTES, chosen as a database is made, raises the
limit. A column of a collation is keyed by its sort key, about 16 bytes for
each character under UNICODE and half as many again under a language's
collation, so its index holds shorter text.
