# Appendix E: Implementation-Defined Choices

[Contents](README.md)

The standards leave some things to each implementation. turborel's
choices:

## By clause of the standard

| What | turborel |
|---|---|
| The character set; its order | Unicode, SQL_TEXT; strings ordered by Unicode scalar value, unless a collation says otherwise |
| LIKE under a collation | by each character's folding; under UNICODE and the collations of languages, each run of the pattern's characters matches a part of the value equal to it |
| The end of a line | `\n` or `\r\n` |
| NUMERIC and DECIMAL of no precision | 18 digits |
| DECIMAL; INTEGER; SMALLINT | as written; NUMERIC(10, 0); NUMERIC(5, 0) |
| FLOAT, REAL, DOUBLE PRECISION | 24 binary digits for FLOAT(p) of p up to 24, else 53; REAL 24; DOUBLE PRECISION and FLOAT 53 |
| DECFLOAT of no precision; the greatest precision | 34 digits; 34 |
| The range of DECFLOAT | an exponent of its leading digit from -6143 to 6144 |
| Rounding to DECFLOAT | half to even |
| An arithmetic result with a DECFLOAT operand | DECFLOAT(34) |
| A DECFLOAT value as text | plain, unless it has more than 34 digits before the point or 6 zeros after it, then in E notation |
| The precision of COUNT and SUM; the scale of AVG | 18; the argument's and 2 more; of DECFLOAT, DECFLOAT(34) |
| A quotient of exact numbers | 18 digits, at the largest scale that fits, truncated |
| A number stored in an exact type of a smaller scale | an exact one or a DECFLOAT truncated; an approximate one rounded |
| Where nulls sort | after every value: last ascending, first descending |
| The national character set | SQL_TEXT: NATIONAL CHARACTER is CHARACTER |
| The longest fraction of a second | 6 digits, microseconds |
| An interval's leading field | at most 9 digits |
| The difference of two datetimes | truncated, toward 0, to its least field |
| A session's time zone | the setting SESSION.TIME_ZONE, +00:00 unless set |
| CURRENT_USER, SESSION_USER, SYSTEM_USER | each the session's authorization identifier |
| The default schema | the session's authorization identifier, until SET SCHEMA |
| The catalog's name | the setting CATALOG.NAME, TURBOREL unless set |
| What a CREATE SCHEMA needs | a session's own schema, its AUTHORIZATION its own identifier |
| Whether a session may SET SESSION AUTHORIZATION | a session begun by the database's owner, to any; any other, only to its own |
| The isolation levels | SERIALIZABLE as itself; REPEATABLE READ as snapshot isolation; READ COMMITTED and READ UNCOMMITTED with a snapshot of each statement |
| A session's end with a transaction open | COMMIT, or ROLLBACK, as the setting SESSION.COMMIT_AT_END says |
| The diagnostics area | one condition, of the last statement; MORE is N |
| An SQL-server's name | an address, 'host:port' or a socket's path |
| A view | not materialized: its query is read into each query that reads it |
| A constraint not named | named for its table, its kind and a number: `T_PRIMARY1` |
| Which identifiers are users | any, of a program's own session; of a server's client, the users the database keeps |

[CONFORMANCE.md](../../CONFORMANCE.md) lists each choice with the rule
it is of.
