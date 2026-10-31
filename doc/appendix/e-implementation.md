# Appendix E: Implementation-Defined Choices

[Contents](README.md)

The standards leave some things to each implementation. turborel's
choices:

## By clause of the standard

| What | turborel |
|---|---|
| The character set; its order | Unicode, SQL_TEXT; strings ordered by Unicode scalar value |
| The end of a line | `\n` or `\r\n` |
| NUMERIC and DECIMAL of no precision | 18 digits |
| DECIMAL; INTEGER; SMALLINT | as written; NUMERIC(10, 0); NUMERIC(5, 0) |
| FLOAT, REAL, DOUBLE PRECISION | 24 binary digits for FLOAT(p) of p up to 24, else 53; REAL 24; DOUBLE PRECISION and FLOAT 53 |
| The precision of COUNT and SUM; the scale of AVG | 18; the argument's and 2 more |
| A quotient of exact numbers | 18 digits, at the largest scale that fits, truncated |
| A number stored in an exact type of a smaller scale | an exact one truncated; an approximate one rounded |
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
