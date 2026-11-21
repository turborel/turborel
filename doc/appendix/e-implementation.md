# Appendix E: Implementation-Defined Choices

[Contents](README.md)

The standards leave some things to each implementation. turborel's
choices:

## By clause of the standard

| What | Turborel |
|---|---|
| The character set; its order | Unicode, SQL_TEXT; strings ordered by Unicode scalar value, unless a collation says otherwise |
| LIKE under a collation | by each character's folding; under UNICODE and the collations of languages, each run of the pattern's characters matches a part of the value equal to it |
| The end of a line | `\n` or `\r\n` |
| NUMERIC and DECIMAL of no precision | 18 digits |
| DECIMAL; INTEGER; SMALLINT; BIGINT | as written; NUMERIC(10, 0); NUMERIC(5, 0); NUMERIC(18, 0) |
| FLOAT, REAL, DOUBLE PRECISION | 24 binary digits for FLOAT(p) of p up to 24, else 53; REAL 24; DOUBLE PRECISION and FLOAT 53 |
| DECFLOAT of no precision; the greatest precision | 34 digits; 34 |
| The range of DECFLOAT | an exponent of its leading digit from -6143 to 6144 |
| Rounding to DECFLOAT | half to even |
| An arithmetic result with a DECFLOAT operand | DECFLOAT(34) |
| A DECFLOAT value as text | plain, unless it has more than 34 digits before the point or 6 zeros after it, then in E notation |
| The precision of COUNT and SUM; the scale of AVG | 18; the argument's and 2 more; of DECFLOAT, DECFLOAT(34) |
| A quotient of exact numbers | Of (p1, s1) by (p2, s2), p1 - s1 + s2 integer digits and scale max(6, s1 + p2 + 1), fewer as the integer digits need, truncated |
| A sum, difference or product of exact numbers | the digits its operands' types give, at most 38 |
| SUM and AVG of exact numbers | 38 digits |
| A number stored in an exact type of a smaller scale | an exact one or a DECFLOAT truncated; an approximate one rounded |
| Where nulls sort | after every value: last ascending, first descending |
| The national character set | SQL_TEXT: NATIONAL CHARACTER is CHARACTER |
| The longest fraction of a second | 6 digits, microseconds |
| An interval's leading field | at most 9 digits |
| The difference of two datetimes | truncated, toward 0, to its least field |
| A session's time zone | the setting SESSION.TIME_ZONE, +00:00 unless set |
| The least and greatest time zone displacements | -12:59 and +14:00 |
| CURRENT_USER, SESSION_USER, SYSTEM_USER | CURRENT_USER the session's authorization identifier, or null in a trigger or routine of a schema a role owns; SESSION_USER and SYSTEM_USER the session's user |
| Who may create a role | the database's owner, and each user it allows by ALTER USER ... CREATE ROLE |
| The default schema | the session's authorization identifier, until SET SCHEMA |
| The catalog's name | the setting CATALOG.NAME, TURBOREL unless set |
| What a CREATE SCHEMA needs | a session's own schema, its AUTHORIZATION its own identifier or the role it has set |
| Whether a session may SET SESSION AUTHORIZATION | a session begun by the database's owner, to any; any other, only to its own |
| The isolation levels | SERIALIZABLE as itself; REPEATABLE READ as snapshot isolation; READ COMMITTED and READ UNCOMMITTED with a snapshot of each statement |
| A session's end with a transaction open | COMMIT, or ROLLBACK, as the setting SESSION.COMMIT_AT_END says |
| The most savepoints of a transaction | 1,024 |
| ROLLBACK TO SAVEPOINT: cursors, prepared statements, diagnostics | every open cursor closed, one opened before the savepoint too; no prepared statement deallocated; the diagnostics area tells of the ROLLBACK as of any statement |
| The diagnostics area | one condition, of the last statement; MORE is N |
| An SQL-server's name | an address, 'host:port' or a socket's path |
| A view | not materialized: its query is read into each query that reads it |
| A constraint not named | named for its table, its kind and a number: `T_PRIMARY1` |
| The null treatment of a UNIQUE constraint or predicate that writes none | NULLS DISTINCT |
| Two rows all null, under NULLS NOT DISTINCT | the same: a key refuses the second, and UNIQUE is false of them |
| A sequence generator of no type; its bounds unwritten | NUMERIC(18); ascending, 1 to the type's greatest value, descending, its least to -1 |
| The values a crash skips of a sequence generator | at most 1024, those reserved and not given out |
| A recursive query with no fixpoint | refused after EXECUTOR.MAX_ITERATIONS iterations, a million unless the session sets another, 0 for no limit |
| How a recursive query is evaluated | over every row each iteration, as the standard says, or over only the rows the last iteration added where that gives the same rows |
| The order of a recursive query's rows | as they are found, unless ORDER BY orders them |
| An array type's maximum cardinality written without one; the greatest | 65,535; 65,535 |
| The names of a row's fields made of values, as by ROW(...) | F1, F2 and so on, by place, described as UNNAMED |
| UNNEST's ordinality column | INTEGER |
| A distinct type's source of an exact or an approximate type | NUMERIC of its precision and scale; FLOAT of its precision |
| The SQL-path, where a function's name not qualified is found | the schema of the statement's names, the session's default schema or a schema statement's own |
| A session's transform group of a distinct type | the one its definition makes, so a value goes to and from the host as its source's |
| The order of ARRAY_AGG's elements without ORDER BY | as the rows are read |
| Which identifiers are users | any, of a program's own session; of a server's client, the users the database keeps |
| JSON_VALUE and JSON_QUERY of no RETURNING | CHARACTER VARYING(32767) |
| A PASSING argument of an interval | cast to CHARACTER VARYING(32767) |
| The JSON text of a context item or a FORMAT JSON argument | parsed WITHOUT UNIQUE KEYS before the path is evaluated, so text that is not JSON is SQLSTATE 22032 whether the path reads it or not |
| Members of one key in JSON text | all kept, in the order written; a member accessor takes the first, `.*` and keyvalue() give all, and none raises an exception |
| A subscript that is not a whole number | truncated toward zero |
| The type of number() | a number's own; of a string, that of the numeric literal it is |
| The type of string() | CHARACTER VARYING(32767) |
| keyvalue()'s `id` | the object's place in the sequence keyvalue() is applied to, from 0 |
| A comparison, like_regex or starts with of one item in error and one that succeeds | True in lax mode, Unknown in strict mode |
| Which error datetime() raises of several items in error | the first item's |
| A path whose evaluation must raise an exception, as `strict $[last+1]` | accepted, the exception raised as it is evaluated |
| An SQL/JSON item as JSON text | no white space between tokens; members in the order kept; only `"`, `\` and control characters escaped; a DECFLOAT without an exponent |
| JSON_OBJECT, JSON_ARRAY, JSON_OBJECTAGG and JSON_ARRAYAGG of no RETURNING | CHARACTER VARYING(32767) |
| A constructor's input of a type it does not keep, as a date | cast to CHARACTER VARYING(32767) |
| An element FORMAT JSON of JSON_ARRAY or JSON_ARRAYAGG | parsed WITHOUT UNIQUE KEYS |
| The order of JSON_ARRAYAGG's elements without ORDER BY | as the rows are read |

[CONFORMANCE.md](../../CONFORMANCE.md) lists each choice with the rule
it is of.
