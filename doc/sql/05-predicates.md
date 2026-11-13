# Predicates and Search Conditions

[Contents](README.md)

A predicate is true, false or **unknown**: unknown where a null makes
the answer unknown. A WHERE, a HAVING, an ON, a CHECK or a WHEN keeps a
row only where its condition is true; a CHECK refuses a row only where
its condition is false. The examples are of the table STAFF of
[Value Expressions](04-expressions.md): Ada, ENG, 7200; Grace, ENG,
8100; Edsger, OPS, 5400; and Barbara, of no department or salary.

## Comparison

`=`, `<>`, `<`, `>`, `<=` and `>=` compare two values of comparable
types: numbers with numbers, strings with strings, datetimes of the same
fields, intervals of one class. A comparison with a null is unknown.
Strings compare as if the shorter were padded with spaces.

Rows of values compare too, `(a, b) = (x, y)`: equal if each pair is;
ordered by the first pair that differs.

```sql
SELECT ID FROM STAFF WHERE SALARY > 7000 ORDER BY ID;
SELECT ID FROM STAFF WHERE (DEPT, SALARY) = ('ENG', 8100);
SELECT ID FROM STAFF WHERE NAME = 5;
```

```
           ID
--------------
            1
            2
(2 rows)
           ID
--------------
            2
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): CHARACTER VARYING(30) and NUMERIC(1,0) are not comparable [ISO/IEC 9075:1992 §8.2 SR1, SR3] (at character 27)
```

Barbara's null salary is neither above 7000 nor not.

## BETWEEN

`x BETWEEN a AND b` is `x >= a AND x <= b`; NOT BETWEEN its negation.

```sql
SELECT ID FROM STAFF WHERE SALARY BETWEEN 5000 AND 7500 ORDER BY ID;
```

```
           ID
--------------
            1
            3
(2 rows)
```

## IN

`x IN (a, b, ...)` is true if x equals one of the values; `x IN
(subquery)` if it equals one of the subquery's. NOT IN is its
negation: unknown if x equals none and one of them is null.

```sql
SELECT ID FROM STAFF WHERE DEPT IN ('OPS', 'HR') ORDER BY ID;
```

```
           ID
--------------
            3
(1 row)
```

## LIKE

`s LIKE pattern [ESCAPE c]` matches a string against a pattern: `_`
any one character, `%` any characters, none among them; the escape
character before one of them, or itself, takes it as written. The
pattern is matched to the whole string, case and spaces as they are.

```sql
SELECT ID FROM STAFF WHERE NAME LIKE '_d%' ORDER BY ID;
SELECT ID FROM STAFF WHERE NAME LIKE '%!%%' ESCAPE '!';
```

```
           ID
--------------
            1
            3
(2 rows)
           ID
--------------
(0 rows)
```

Under a collation LIKE compares folded characters, so
`NAME LIKE 'b%'` finds `Bob` in a CASE_INSENSITIVE column. Under UNICODE and
a language's collation, whose letters may be several characters, each run of
the pattern's characters matches a part of the value equal to it under the
collation. So `WORD LIKE 'å%'` finds `åka` but not `apa` under UNICODE_SV,
and under UNICODE_JA, which ignores punctuation and the kana's script,
`'かな'` matches `カ-ナ`.

## LIKE_REGEX

`s LIKE_REGEX pattern [FLAG flag]` is true if some part of s matches an
XQuery regular expression; `NOT LIKE_REGEX` if none does. Unlike LIKE,
the pattern need not match the whole string: `^` and `$` anchor it at
the ends. The flag is letters, each turning an option on: `i` ignores
case, `s` lets `.` match a newline, `m` makes `^` and `$` match at each
line, `x` ignores white space in the pattern, and `q` takes every
character of the pattern as itself. The pattern and flag may come from
columns or parameters. It is unknown if any operand is null.

```sql
SELECT ID FROM STAFF WHERE NAME LIKE_REGEX '^[AE]' ORDER BY ID;
SELECT ID FROM STAFF WHERE NAME LIKE_REGEX 'a$' FLAG 'i'
  AND NAME NOT LIKE_REGEX '(.)\1' ORDER BY ID;
SELECT ID FROM STAFF WHERE NAME LIKE_REGEX '(';
```

```
           ID
--------------
            1
            3
(2 rows)
           ID
--------------
            1
            4
(2 rows)
turborel: SQLCODE -301 (SQLSTATE 2201S): the pattern is not an XQuery regular expression: a '(' is not closed (at character 1) [ISO/IEC 9075-2:2023 §9.28 GR4]
```

A pattern that is no regular expression is SQLSTATE 2201S, a flag of
any other letter 2201T. Matching takes no account of a collation, so a
CASE_INSENSITIVE column needs the flag `i`. A newline is any of LF, VT,
FF, CR, NEL, LINE SEPARATOR, PARAGRAPH SEPARATOR and CRLF, which counts
as one. [Limits](../appendix/d-limits.md) gives the size a pattern may
have.

## NULL

`x IS NULL` is true if x is null; `IS NOT NULL` if it is not. It is
never unknown. Of a row of values, IS NULL is true if each value is
null, IS NOT NULL if none is.

```sql
SELECT ID FROM STAFF WHERE DEPT IS NULL;
```

```
           ID
--------------
            4
(1 row)
```

## DISTINCT

`x IS DISTINCT FROM y` is true if x and y differ, a null counting as a
value. Two nulls are not distinct, and a null and a value are. It is
never unknown, so `IS NOT DISTINCT FROM` is the comparison to use where
a null should equal a null. Rows are distinct if a pair of their values
is, and arrays if their cardinalities differ or a pair of elements at
one place is. The operands must be comparable, as for `=`. A bare NULL
is no operand, so write `CAST(NULL AS type)`.

```sql
SELECT ID FROM STAFF WHERE SALARY IS DISTINCT FROM 7200 ORDER BY ID;
SELECT ID FROM STAFF WHERE DEPT IS NOT DISTINCT FROM CAST(NULL AS CHAR(3));
```

```
           ID
--------------
            2
            3
            4
(3 rows)
           ID
--------------
            4
(1 row)
```

Barbara's null salary is distinct from 7200, where `SALARY <> 7200`
would leave her out. A join on `a IS NOT DISTINCT FROM b` is hashed on
it as on `a = b`, a null key joining a null key.

## Quantified comparison

`x op ALL (subquery)` is true if the comparison is true of every row of
the subquery, and of none; `x op ANY (subquery)`, or SOME, if it is
true of one. ANY or SOME before anything but a subquery is the set
function of truth values, so `x = ANY(y)` compares x with it.

```sql
SELECT ID FROM STAFF WHERE SALARY > ALL (SELECT SALARY FROM STAFF WHERE DEPT = 'OPS') ORDER BY ID;
SELECT ID FROM STAFF WHERE SALARY > ANY (SELECT SALARY FROM STAFF WHERE DEPT = 'ENG') ORDER BY ID;
```

```
           ID
--------------
            1
            2
(2 rows)
           ID
--------------
            2
(1 row)
```

## EXISTS

`EXISTS (subquery)` is true if the subquery has a row. It is never
unknown. The subquery's rows are read only up to the first its
condition keeps, so an error its condition would raise of a later row,
such as a division by zero, is not raised.

```sql
SELECT ID FROM STAFF S WHERE EXISTS (SELECT * FROM STAFF T WHERE T.DEPT = S.DEPT AND T.ID <> S.ID) ORDER BY ID;
```

```
           ID
--------------
            1
            2
(2 rows)
```

## UNIQUE

`UNIQUE (subquery)` is true if no two rows of the subquery are equal,
rows with a null not counting. `UNIQUE NULLS DISTINCT (subquery)` is the
same. `UNIQUE NULLS NOT DISTINCT (subquery)` counts them too, a null
equal to a null: it is false if two rows are equal but for nulls in the
same columns, two rows all null among them.

```sql
SELECT COUNT(*) FROM STAFF WHERE UNIQUE (SELECT DEPT FROM STAFF WHERE DEPT IS NOT NULL);
```

```

----------------------
                    0
(1 row)
```

Two rows have ENG, so it is false of every row.

Barbara's department is null. Two rows of it, each a null, are not
counted by UNIQUE, but are equal under NULLS NOT DISTINCT:

```sql
SELECT COUNT(*) FROM STAFF
    WHERE UNIQUE (SELECT DEPT FROM STAFF WHERE DEPT IS NULL UNION ALL SELECT DEPT FROM STAFF WHERE DEPT IS NULL);
SELECT COUNT(*) FROM STAFF
    WHERE UNIQUE NULLS NOT DISTINCT (SELECT DEPT FROM STAFF WHERE DEPT IS NULL UNION ALL SELECT DEPT FROM STAFF WHERE DEPT IS NULL);
```

```

----------------------
                    4
(1 row)

----------------------
                    0
(1 row)
```

## MATCH

`row MATCH [UNIQUE] [PARTIAL | FULL] (subquery)` is true if the row
matches a row of the subquery, and with UNIQUE exactly one. With
neither PARTIAL nor FULL, a row with a null matches; FULL, a row all
null, and one of no null that equals a row; PARTIAL, a row whose values
not null equal a row's. It is never unknown. It is the test a foreign
key makes of its MATCH.

```sql
SELECT ID FROM STAFF WHERE (DEPT) MATCH FULL (SELECT DEPT FROM STAFF WHERE ID = 1);
```

```
           ID
--------------
            1
            2
            4
(3 rows)
```

## OVERLAPS

`(start1, end1) OVERLAPS (start2, end2)` is true if two periods of time
share an instant; each period's end may be given as an interval from its
start.

```sql
VALUES CASE WHEN (DATE '2026-01-01', DATE '2026-02-01') OVERLAPS (DATE '2026-01-15', INTERVAL '1' DAY)
            THEN 'yes' ELSE 'no' END;
```

```

-----
 yes
(1 row)
```

## Search conditions

Predicates are joined by AND, OR and NOT, NOT binding tightest, then
AND; parentheses group. Of true, false and unknown:

| a | b | a AND b | a OR b | NOT a |
|---|---|---|---|---|
| true | unknown | unknown | true | false |
| false | unknown | false | unknown | true |
| unknown | unknown | unknown | unknown | unknown |

`c IS [NOT] TRUE`, `FALSE` or `UNKNOWN` tests a condition's truth, and
is never unknown itself.

```sql
SELECT ID FROM STAFF WHERE (SALARY > 7000) IS UNKNOWN;
SELECT ID FROM STAFF WHERE NOT (SALARY > 7000) ORDER BY ID;
SELECT ID FROM STAFF WHERE SALARY > 7000 OR DEPT = 'OPS' ORDER BY ID;
```

```
           ID
--------------
            4
(1 row)
           ID
--------------
            3
(1 row)
           ID
--------------
            1
            2
            3
(3 rows)
```

NOT kept neither Barbara's row, its salary's comparison unknown, nor so
its negation.

A value of type [BOOLEAN](02-data-types.md#booleans) is a condition of
its own, so `WHERE DONE` keeps the rows where the column DONE is TRUE. A
value of any other type there is refused, SQLCODE -103. A condition is
also a value ([Value Expressions](04-expressions.md#conditions-as-values)),
so `(SALARY > 7000) IS NULL` is true where the comparison is unknown.

The standard: ISO/IEC 9075:1992 §8.1–§8.12; X3.135-1986 §5.10–§5.18;
ISO/IEC 9075-2:2023 §6.46 for a BOOLEAN value as a condition.
