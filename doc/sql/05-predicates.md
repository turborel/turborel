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

## Quantified comparison

`x op ALL (subquery)` is true if the comparison is true of every row of
the subquery, and of none; `x op ANY (subquery)`, or SOME, if it is
true of one.

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
unknown.

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
rows with a null not counting.

```sql
SELECT COUNT(*) FROM STAFF WHERE UNIQUE (SELECT DEPT FROM STAFF WHERE DEPT IS NOT NULL);
```

```

----------------------
                    0
(1 row)
```

Two rows have ENG, so it is false of every row.

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
its negation. A condition is no value: SQL-92 has no BOOLEAN type, so a
comparison is not selected nor stored.

The standard: ISO/IEC 9075:1992 §8.1–§8.12; X3.135-1986 §5.10–§5.18.
