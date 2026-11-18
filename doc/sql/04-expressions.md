# Value Expressions

[Contents](README.md)

A value expression computes a value: of a column, a literal, an
operator, a function or a subquery. The examples here are of a table
STAFF:

```sql
CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), DEPT CHAR(3), SALARY DECIMAL(9,2));
INSERT INTO STAFF VALUES (1, 'Ada', 'ENG', 7200), (2, 'Grace', 'ENG', 8100),
                         (3, 'Edsger', 'OPS', 5400), (4, 'Barbara', NULL, NULL);
```

```
OK
OK
```

## Value specifications

The simplest values are:

- a literal, `42`, `'Ada'` or `TRUE` ([Lexical Elements](01-lexical.md));
- a column, `NAME` or `S.NAME` ([Names](03-names.md));
- USER, the same as CURRENT_USER, the session's authorization
  identifier; CURRENT_ROLE, the role the session has set (see
  [SET ROLE](statements/set-role.md)), null if none; SESSION_USER and
  SYSTEM_USER, which are the same here, the session's user. In a trigger
  or a routine of a schema a role owns, CURRENT_USER is null and
  CURRENT_ROLE the role, while SESSION_USER stays the session's;
- CURRENT_DATE, CURRENT_TIME(p) and CURRENT_TIMESTAMP(p), the instant
  the statement began, the last two WITH TIME ZONE, in the session's;
- in a program, a parameter, or a `?` of a prepared statement;
- a scalar subquery: a query of one column, in parentheses, its value
  that of its one row, or null if it has none. A second row is refused.

```sql
VALUES (USER, CURRENT_USER, SESSION_USER, SYSTEM_USER);
SELECT NAME FROM STAFF WHERE SALARY = (SELECT MAX(SALARY) FROM STAFF);
SELECT NAME FROM STAFF WHERE ID = (SELECT ID FROM STAFF);
```

```
     |     |     |
-----+-----+-----+-----
 ANN | ANN | ANN | ANN
(1 row)
 NAME
--------------------------------
 Grace
(1 row, with a warning: SQLSTATE 01003)
turborel: SQLCODE -811 (SQLSTATE 21000): a <scalar subquery> or <row subquery> gives more than one row [ISO/IEC 9075:1992 §7.11 GR1]
```

**The null value** is no value: a column of no value, a set function of
no rows. An operator or function of a null is null. NULL is not a value
expression of its own: it is written only where its type is known, as
a value inserted, a SET, a DEFAULT, or `CAST(NULL AS INTEGER)`.

## Numeric expressions

`+`, `-`, `*` and `/` on numbers, and a sign, `-x`. `*` and `/` bind
tighter than `+` and `-`; parentheses group. An expression of exact
numbers is exact, of 18 digits at most; one of an approximate number is
approximate; one of a DECFLOAT is DECFLOAT(34), rounded to 34 digits.
A quotient of exact numbers is at the largest scale that
fits, so 10 / 4 is 2.5; a product's scale is its operands' scales
summed. Division by zero is refused, and so is a result past 18 digits.

```sql
VALUES (1 + 2 * 3, -(4 - 6), 10 / 4, 2.50 * 2, 1E0 / 3);
VALUES (1 / 0);
SELECT ID, SALARY * 1.1 FROM STAFF ORDER BY ID;
```

```
                      |                      |                      |                      |
----------------------+----------------------+----------------------+----------------------+--------------------------
                    7 |                    2 |                  2.5 |                 5.00 |     3.333333333333333E-1
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22012): a divisor shall not be 0 [X3.135-1986 §5.9 GR5]
           ID |
--------------+----------------------
            1 |             7920.000
            2 |             8910.000
            3 |             5940.000
            4 |                 NULL
(4 rows)
```

The numeric functions:

| Function | Value |
|---|---|
| CHAR_LENGTH(s), CHARACTER_LENGTH(s) | the characters of a character string |
| OCTET_LENGTH(s) | the octets of a string, as UTF-8, or of a binary string |
| BIT_LENGTH(s) | *(extension)* eight bits to each of those octets |
| POSITION(a IN b) | where string a first begins in b, from 1; 0 if not in it, 1 if a is empty; of binary strings, in octets |
| EXTRACT(f FROM d) | the field f of a datetime or interval |
| WIDTH_BUCKET(x, b1, b2, n) | which of n buckets of equal width from b1 to b2 x falls in |
| LN(x), EXP(x) | the natural logarithm of x, and e raised to the power x |
| POWER(x, y), SQRT(x) | x raised to the power y, and the square root of x, POWER(x, 0.5) |
| FLOOR(x) | the greatest whole number not above x |
| CEILING(x), CEIL(x) | the least whole number not below x |

WIDTH_BUCKET cuts the range from b1 to b2 into n buckets of equal width
and gives the bucket x falls in, from 1. A value before b1 is in bucket
0, and one at b2 or past it in bucket n + 1. Each bucket holds its lower
edge, so with b1 greater than b2 the buckets count down, and b2 is in
bucket n + 1. The result is null if any operand is. Each operand is a
number, n is exact of scale 0, and the result is of n's type. An n of 0
or below, or equal bounds, are refused (SQLSTATE 2201G), as is an n + 1
past n's type (22003). Of exact operands the bucket is found exactly.

```sql
SELECT NAME, SALARY, WIDTH_BUCKET(SALARY, 5000, 9000, 4) AS UP,
       WIDTH_BUCKET(SALARY, 9000, 5000, 4) AS DOWN
  FROM STAFF ORDER BY NAME;
VALUES (WIDTH_BUCKET(1, 0, 0, 4));
```

```
 NAME                           |      SALARY |  UP | DOWN
--------------------------------+-------------+-----+------
 Ada                            |     7200.00 |   3 |    2
 Barbara                        |        NULL | NULL | NULL
 Edsger                         |     5400.00 |   1 |    4
 Grace                          |     8100.00 |   4 |    1
(4 rows)
turborel: SQLCODE -301 (SQLSTATE 2201G): the width bucket bounds are equal [ISO/IEC 9075-2:2023 §6.31 GR17c]
```

LN, EXP, POWER and SQRT are DOUBLE PRECISION, or DECFLOAT(34) if an
operand is DECFLOAT. FLOOR and CEILING of an exact number are exact of
scale 0, found exactly; of an approximate number or a DECFLOAT they are
of its type. Each is null if an operand is. POWER of 0 to the power 0 is
1, and of a negative number to a whole power is negative only of an odd
power. Of a DECFLOAT, SQRT and a whole power are found to 34 digits, and
LN, EXP and other powers to the 16 digits of double precision.

LN of 0 or below is refused (SQLSTATE 2201E, SQLCODE -301). So is POWER
of 0 to a negative power, or of a negative number to a power not whole,
and so SQRT of a negative number (2201F, -301). A result past the range
of its type is refused too (22003, -301).

```sql
VALUES (LN(1), EXP(1), POWER(2, 10), POWER(-2, 3), SQRT(2));
VALUES (FLOOR(-2.5), CEILING(-2.5), FLOOR(2.5E0), CEIL(CAST(0.001 AS DECFLOAT)), SQRT(CAST(2 AS DECFLOAT)));
VALUES (LN(0));
VALUES (SQRT(-1));
```

```
                          |                          |                          |                          |
--------------------------+--------------------------+--------------------------+--------------------------+--------------------------
                      0E0 |      2.718281828459045E0 |                  1.024E3 |                     -8E0 |     1.4142135623730951E0
(1 row)
      |      |                          |                                             |
------+------+--------------------------+---------------------------------------------+---------------------------------------------
   -3 |   -2 |                      2E0 |                                           1 |         1.414213562373095048801688724209698
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2201E): the natural logarithm is of a number above 0 [ISO/IEC 9075-2:2023 §6.31 GR12b]
turborel: SQLCODE -301 (SQLSTATE 2201F): a negative base is raised only to a whole number [ISO/IEC 9075-2:2023 §6.31 GR14e]
```

## String expressions

`||` joins two strings, or two binary strings; of CHARACTER VARYING, or
BINARY VARYING, if either is. Binary strings joined past 32,767 octets
keep their first ones if the rest are X'00', else are refused (SQLSTATE
22001). The string functions:

| Function | Value |
|---|---|
| SUBSTRING(s FROM start [FOR length] [USING units]) | the characters of s from start, counting from 1, length of them or the rest; of a binary string, its octets |
| UPPER(s), LOWER(s) | s in upper or lower case |
| TRIM([[LEADING \| TRAILING \| BOTH] [c] FROM] s) | s without the character c, a space if not written, at its start, end or both, the default |
| LTRIM(s [, set]), RTRIM(s [, set]), BTRIM(s [, set]) | s without any of the characters of set, a space if not written, at its start, its end or both |
| LPAD(s, n [, pad]), RPAD(s, n [, pad]) | s brought to n characters, cut if longer, else with pad, a space if not written, repeated before it or after it; '' if n is less than 1 |
| OVERLAY(s PLACING r FROM start [FOR length] [USING units]) | s with r in place of length characters from start, or of as many as r has; of binary strings, octets |
| TRIM([[LEADING \| TRAILING \| BOTH] [o] FROM] b) | of a binary string b, b without the octet o, X'00' if not written |

A function of a CHARACTER VARYING string is CHARACTER VARYING, and
SUBSTRING, OVERLAY and TRIM of a binary string BINARY VARYING. Of a
[large object](02-data-types.md#large-objects) each is a large object of
its type, and LPAD and RPAD of one a CLOB of 4G characters. UPPER, LOWER, the trims and
the pads take character strings alone. A trim is as long as its string
at most. A pad is CHARACTER VARYING(32767), and gives at most 32,767
characters, so cast it to a shorter type to show or store it narrow. A
trim compares characters under the collation of its string, so under
CASE_INSENSITIVE `TRIM('b' FROM 'Bob')` is `o`. A trim or pad is of its
string's collation, whatever that of its other strings. LTRIM, RTRIM,
BTRIM, LPAD and RPAD are not reserved, so a column may be named any of
them.

```sql
VALUES ('Ada' || ' ' || 'Lovelace', SUBSTRING('Lovelace' FROM 1 FOR 4), UPPER('ada'),
        POSITION('ve' IN 'Lovelace'), CHAR_LENGTH('Ada'));
VALUES ('[' || TRIM('  x  ') || ']', TRIM(LEADING '0' FROM '007'), OCTET_LENGTH('é'), BIT_LENGTH('A'));
VALUES (LTRIM('xxyAda', 'xy'), RTRIM('Ada...', '.'), '[' || BTRIM('  Ada  ') || ']', CAST(LPAD('42', 6, '0') AS CHAR(6)),
        CAST(RPAD('Ada', 8, '.-') AS CHAR(8)), CAST(LPAD('Lovelace', 4) AS CHAR(4)));
```

```
              |          |     |                      |
--------------+----------+-----+----------------------+----------------------
 Ada Lovelace | Love     | ADA |                    3 |                    3
(1 row)
         |     |                      |
---------+-----+----------------------+----------------------
 [x]     | 7   |                    2 |                    8
(1 row)
        |        |           |        |          |
--------+--------+-----------+--------+----------+------
 Ada    | Ada    | [Ada]     | 000042 | Ada.-.-. | Love
(1 row)
```

```sql
VALUES (POSITION(X'0B' IN X'0A0B0C'), SUBSTRING(X'0A0B0C' FROM 2), X'0A' || X'0B', OCTET_LENGTH(X'0A0B'), BIT_LENGTH(X'0A0B'));
```

```
                      |        |      |                      |
----------------------+--------+------+----------------------+----------------------
                    2 | 0B0C   | 0A0B |                    2 |                   16
(1 row)
```

SUBSTRING, POSITION(a IN s [USING units]) and CHAR_LENGTH(s [USING
units]) count characters, or with USING OCTETS octets of the string's
UTF-8. A substring in octets takes the whole characters within them, so
it never splits one. USING CHARACTERS is the default spelled out, and a
binary string's functions take no USING.

```sql
VALUES (CHAR_LENGTH('café' USING OCTETS), POSITION('crème' IN 'café crème' USING OCTETS), SUBSTRING('café crème' FROM 7 USING OCTETS));
```

```
                      |                      |
----------------------+----------------------+------------
                    5 |                    7 | crème
(1 row)
```

CONVERT and TRANSLATE, of character sets other than SQL_TEXT, are valid
SQL Turborel does not process: SQLCODE -900.

### Regular expressions

Four functions search a string for the matches of an XQuery regular
expression, read as [LIKE_REGEX](05-predicates.md#like_regex) reads it,
with a flag after FLAG if one is written. Matches are counted from the
start position after FROM, 1 if none is written, and each begins after
the one before it ends.

| Function | Value |
|---|---|
| OCCURRENCES_REGEX(pattern [FLAG f] IN s [FROM start] [USING units]) | how many matches there are; -1 if start is not a position of s |
| POSITION_REGEX([START \| AFTER] pattern [FLAG f] IN s [FROM start] [USING units] [OCCURRENCE n] [GROUP g]) | where the nth match, or its group g, begins, or with AFTER the position after it ends; 0 if there is none |
| SUBSTRING_REGEX(pattern [FLAG f] IN s [FROM start] [USING units] [OCCURRENCE n] [GROUP g]) | what the nth match, or its group g, matched; null if there is none |
| TRANSLATE_REGEX(pattern [FLAG f] IN s [WITH r] [FROM start] [USING units] [OCCURRENCE n \| ALL]) | s with every match, or the nth, replaced by r, '' if not written |

The nth match is the first if OCCURRENCE is not written, and group 0 is
the whole match. In the replacement, `$1` stands for what the first
group matched, `$0` for the whole match, and `\$` and `\\` for `$` and
`\`. USING OCTETS counts positions in octets of the string as UTF-8,
USING CHARACTERS, the default, in characters. A function's value is null
if an operand is.

```sql
SELECT ID, OCCURRENCES_REGEX('a' FLAG 'i' IN NAME), POSITION_REGEX('[aeiou]' IN NAME OCCURRENCE 2),
       SUBSTRING_REGEX('r.' IN NAME), CAST(TRANSLATE_REGEX('([aeiou])' IN NAME WITH '<$1>') AS VARCHAR(20))
  FROM STAFF ORDER BY ID;
VALUES (POSITION_REGEX(AFTER 'é' IN 'café crème' USING OCTETS), SUBSTRING_REGEX('(\w+)@(\w+)' IN 'ada@example' GROUP 2),
        CAST(TRANSLATE_REGEX('a' IN 'banana' WITH 'o' OCCURRENCE 2) AS CHAR(6)));
```

```
           ID |                      |                      |                                |
--------------+----------------------+----------------------+--------------------------------+----------------------
            1 |                    2 |                    0 | NULL                           | Ad<a>
            2 |                    1 |                    5 | ra                             | Gr<a>c<e>
            3 |                    0 |                    0 | NULL                           | Edsg<e>r
            4 |                    3 |                    5 | rb                             | B<a>rb<a>r<a>
(4 rows)
                      |             |
----------------------+-------------+--------
                    6 | example     | banona
(1 row)
```

SUBSTRING_REGEX is as long as its string at most, and TRANSLATE_REGEX
too with no replacement, else CHARACTER VARYING(32767), so cast it to
show it narrow. Both are of their string's collation. A replacement of
a match of no characters is SQLSTATE 2201U, and a `$` before no digit
or a `\` before neither `$` nor `\` is 2201V. The four names are not
reserved, so a column may be named any of them.

## Datetime and interval expressions

A datetime plus or minus an interval of its fields; an interval plus
or minus another of its class; an interval times or divided by a number;
the difference of two datetimes, in parentheses, given an interval
qualifier: `(D2 - D1) DAY`. A datetime may be given in another time
zone: `TS AT TIME ZONE INTERVAL '+02:00' HOUR TO MINUTE`, or `TS AT
LOCAL`, the session's. [Data Types](02-data-types.md) shows them, and
what is refused.

## CASE, NULLIF and COALESCE

A searched CASE gives the result of the first WHEN whose condition is
true, else of its ELSE, else null; a simple CASE compares one value with
each WHEN's. A simple CASE's WHEN may list several values, separated by
commas, and is true if any of them equals the value. NULLIF(a, b) is null if a equals b, else a. COALESCE(a, b,
...) is the first of its values not null. Their results are of a type
that holds each result's: strings of the longest, numbers of the most
digits; values of types that do not mix are refused.

```sql
SELECT ID,
       CASE WHEN SALARY > 7000 THEN 'high' WHEN SALARY IS NULL THEN 'unknown' ELSE 'low' END AS BAND,
       CASE DEPT WHEN 'ENG' THEN 'Engineering' WHEN 'OPS', 'SUP' THEN 'Operations' END AS DEPT_NAME,
       NULLIF(DEPT, 'ENG') AS NOT_ENG,
       COALESCE(DEPT, '---') AS SHOWN
  FROM STAFF ORDER BY ID;
```

```
           ID | BAND    | DEPT_NAME   | NOT_ENG | SHOWN
--------------+---------+-------------+---------+-------
            1 | high    | Engineering | NULL    | ENG
            2 | high    | Engineering | NULL    | ENG
            3 | low     | Operations  | OPS     | OPS
            4 | unknown | NULL        | NULL    | ---
(4 rows)
```

A simple CASE's WHEN may also be the second part of a predicate, its
first part the CASE's value: a comparison such as `> 8000`, BETWEEN, IN,
LIKE, IS NULL, a quantified comparison or MATCH, with NOT where the
predicate takes one. Values and predicates mix in one list. The CASE's
value may be a row, such as `(A, B)`, compared with rows, and a pair of
datetimes is compared by OVERLAPS, every WHEN then being OVERLAPS.

```sql
SELECT ID,
       CASE SALARY WHEN > 8000 THEN 'top' WHEN BETWEEN 5000 AND 8000 THEN 'mid' WHEN IS NULL THEN 'unpaid' END AS PAY,
       CASE NAME WHEN LIKE 'A%', LIKE 'B%' THEN 'early' ELSE 'late' END AS ALPHABET
  FROM STAFF ORDER BY ID;
```

```
           ID | PAY    | ALPHABET
--------------+--------+----------
            1 | mid    | early
            2 | top    | late
            3 | mid    | late
            4 | unpaid | early
(4 rows)
```

## GREATEST and LEAST

GREATEST(a, b, ...) is the greatest of two values or more, LEAST(a, b,
...) the least. Each value is first taken to a type that holds them all,
as a CASE's results are, and the values must be comparable. If any is
null, the result is null. Strings are ordered by their collation, and of
two it holds equal the first is the result. GREATEST and LEAST are not
reserved, so a column may be named either.

```sql
SELECT ID, GREATEST(SALARY, 6000) AS FLOOR_PAY, LEAST(SALARY, 8000) AS CAPPED, LEAST(NAME, 'Bob') AS FIRST_NAME
  FROM STAFF ORDER BY ID;
VALUES GREATEST(1, 'one');
```

```
           ID |   FLOOR_PAY |      CAPPED | FIRST_NAME
--------------+-------------+-------------+--------------------------------
            1 |     7200.00 |     7200.00 | Ada
            2 |     8100.00 |     8000.00 | Bob
            3 |     6000.00 |     5400.00 | Bob
            4 |        NULL |        NULL | Barbara
(4 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): the arguments of GREATEST are each comparable with each, not NUMERIC(1,0), CHARACTER(3) [ISO/IEC 9075-2:2023 §6.15 SR4] (at character 7)
```

## Set functions

A set function computes one value of a set of rows: of the whole table
a query reads, or of each group of its GROUP BY.

| Function | Value |
|---|---|
| COUNT(*) | the rows |
| COUNT(x) | the rows where x is not null |
| SUM(x), AVG(x) | the sum and mean of x; of exact numbers, AVG of the scale 2 more; of DECFLOAT, DECFLOAT(34); of intervals too |
| MIN(x), MAX(x) | the least and greatest x |
| EVERY(x) | of a BOOLEAN x, TRUE if each x is TRUE, else FALSE |
| ARRAY_AGG(x ORDER BY ...) | an array of every x, nulls among them, in the order ORDER BY gives; null of no rows |
| COLLECT(x) | a multiset of every x, nulls among them; DISTINCT once each; null of no rows |
| FUSION(m) | a multiset of the elements of every multiset m; null of no rows |
| INTERSECTION(m) | a multiset of as many of each value as the multiset m of fewest has; null of no rows |
| ANY(x), SOME(x) | of a BOOLEAN x, TRUE if an x is TRUE, else FALSE |
| ANY_VALUE(x) | one of the values of x, the first one read |
| LISTAGG(x, 'sep') WITHIN GROUP (ORDER BY ...) | the strings x joined by the separator in the order ORDER BY gives, CHARACTER VARYING(32767) |
| JSON_ARRAYAGG(x [ORDER BY ...]) | the JSON text of an array of every x, in the order ORDER BY gives; null of no rows |
| JSON_OBJECTAGG(name VALUE x) | the JSON text of an object of each row's member, name a character string; null of no rows |
| PERCENTILE_CONT(p) WITHIN GROUP (ORDER BY x) | the value a fraction p of the way through the sorted x, between two of them if it falls between |
| PERCENTILE_DISC(p) WITHIN GROUP (ORDER BY x) | the first of the sorted x whose share of the values up to it is at least p |
| VAR_POP(x), VAR_SAMP(x) | the variance of x, of the population and of a sample |
| STDDEV_POP(x), STDDEV_SAMP(x) | their square roots, the standard deviations |
| COVAR_POP(y, x), COVAR_SAMP(y, x) | the covariance of the pairs of y and x, of the population and of a sample |
| CORR(y, x) | the size of their coefficient of correlation, from 0 to 1 |
| REGR_SLOPE(y, x), REGR_INTERCEPT(y, x) | the slope and intercept of the least-squares line of y on x |
| REGR_R2(y, x) | that line's coefficient of determination |
| REGR_COUNT(y, x) | the pairs in which neither y nor x is null |
| REGR_AVGX(y, x), REGR_AVGY(y, x) | the means of x and of y |
| REGR_SXX(y, x), REGR_SYY(y, x), REGR_SXY(y, x) | the sums of the squares of the deviations of x and of y from their means, and of their products |

Each but COUNT(*) leaves out the nulls of x, and with DISTINCT counts
each value of x once: `COUNT(DISTINCT DEPT)`. Of no rows, COUNT is 0
and the others null. A query whose set function left out a null
completes with a warning, SQLSTATE 01003, null value eliminated in set
function: so does the scalar subquery above, as Barbara's salary is
null.

SUM is refused, SQLSTATE 22003, only if its total does not fit its
type, whatever order the rows are read in: past 18 digits of exact
numbers, the range of approximate ones or of DECFLOAT, or an interval's
leading field.
So 999999999999999999, 1 and -1 sum to 999999999999999999. AVG is
refused only if the mean does not fit. Its digits past its scale are
dropped, not rounded, as those of every exact quotient are, so the mean
of 10.00, 10.00 and 12.00 is 10.6666. A SUM of DECFLOAT values is
rounded to 34 digits after each row, so of values whose total needs
more its last digit may depend on that order.

```sql
SELECT AVG(X) FROM (VALUES (10.00), (10.00), (12.00)) AS V (X);
```

```

----------------------
              10.6666
(1 row)
```

```sql
SELECT COUNT(*), COUNT(DEPT), COUNT(DISTINCT DEPT), SUM(SALARY), AVG(SALARY), MIN(NAME), MAX(SALARY) FROM STAFF;
SELECT DEPT, COUNT(*), AVG(SALARY) FROM STAFF GROUP BY DEPT ORDER BY DEPT;
SELECT SUM(SALARY) FROM STAFF WHERE 1 = 0;
```

```
                      |                      |                      |                      |                      |                                |
----------------------+----------------------+----------------------+----------------------+----------------------+--------------------------------+-------------
                    4 |                    3 |                    2 |             20700.00 |            6900.0000 | Ada                            |     8100.00
(1 row, with a warning: SQLSTATE 01003)
 DEPT |                      |
------+----------------------+----------------------
 ENG  |                    2 |            7650.0000
 OPS  |                    1 |            5400.0000
 NULL |                    1 |                 NULL
(3 rows, with a warning: SQLSTATE 01003)

----------------------
                 NULL
(1 row)
```

A query of a set function and no GROUP BY or HAVING gives one row,
even of no rows, so each column it selects is in a set function, or of
one value in all the rows, as WHERE makes a column it compares with a
literal. A set function is not in a WHERE clause, which is applied to
rows one by one:

```sql
SELECT NAME, COUNT(*) FROM STAFF;
SELECT ID FROM STAFF WHERE SALARY > AVG(SALARY);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): when the select list has a set function, every column reference must be inside one, or of a column functionally dependent on none (found STAFF.NAME) [X3.135-1986 §5.25 SR7; ISO/IEC 9075-2:2023 §6.7 SR8] (at character 7)
turborel: SQLCODE -103 (SQLSTATE 2A000): a <set function specification> is in a <where clause> only in a <having clause>'s <subquery>, of an outer reference [X3.135-1986 §5.21 SR3] (at character 36)
```

The statistical set functions are of numbers. Each is DOUBLE
PRECISION, or DECFLOAT(34) if an argument is DECFLOAT, but REGR_COUNT,
which is exact. VAR_POP, VAR_SAMP, STDDEV_POP and STDDEV_SAMP take no
DISTINCT or ALL (SQLSTATE 2A000, SQLCODE -103). Those of two arguments,
y and x, leave out each row in which either is null, with the warning
01003. Of no rows REGR_COUNT is 0 and the others null; of one row
VAR_SAMP, STDDEV_SAMP and COVAR_SAMP are null, the variance of a sample
of one being unknown. Where every x is alike, CORR, REGR_R2, REGR_SLOPE
and REGR_INTERCEPT are null, as the line would be upright; where every y
is alike and the x are not, CORR is null and REGR_R2 1. CORR is the size
of the coefficient, not its sign. The standard defines it as the square
root of its square, so of y falling as x rises it is 1, not -1, and
REGR_SLOPE gives the sign. A sum on the way past the range of the
result's type is refused (22003, -301).

```sql
SELECT VAR_POP(SALARY), VAR_SAMP(SALARY), STDDEV_POP(SALARY), STDDEV_SAMP(SALARY) FROM STAFF;
SELECT DEPT, REGR_COUNT(SALARY, ID), REGR_SLOPE(SALARY, ID), VAR_SAMP(SALARY)
  FROM STAFF GROUP BY DEPT ORDER BY DEPT;
SELECT REGR_SLOPE(Y, X), REGR_INTERCEPT(Y, X), REGR_R2(Y, X), CORR(Y, X), COVAR_SAMP(Y, X)
  FROM (VALUES (2, 1), (4, 2), (5, 3), (9, 4)) AS P (Y, X);
SELECT VAR_POP(DISTINCT SALARY) FROM STAFF;
```

```
                          |                          |                          |
--------------------------+--------------------------+--------------------------+--------------------------
                   1.26E6 |                   1.89E6 |     1.1224972160321825E3 |      1.374772708486752E3
(1 row, with a warning: SQLSTATE 01003)
 DEPT |                      |                          |
------+----------------------+--------------------------+--------------------------
 ENG  |                    2 |                      9E2 |                   4.05E5
 OPS  |                    1 |                     NULL |                     NULL
 NULL |                    0 |                     NULL |                     NULL
(3 rows, with a warning: SQLSTATE 01003)
                          |                          |                          |                          |
--------------------------+--------------------------+--------------------------+--------------------------+--------------------------
                    2.2E0 |                    -5E-1 |     9.307692307692307E-1 |     9.647638212377321E-1 |     3.6666666666666665E0
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): VAR_POP takes no <set quantifier>: neither DISTINCT nor ALL [ISO/IEC 9075-2:2023 §10.9 SR2] (at character 15)
```

The mean is found by a subquery: `WHERE SALARY > (SELECT AVG(SALARY)
FROM STAFF)`. A column WHERE fixes may be selected:

```sql
SELECT DEPT, COUNT(*) AS N FROM STAFF WHERE DEPT = 'ENG';
```

```
 DEPT |                    N
------+----------------------
 ENG  |                    2
(1 row)
```

A set function followed by OVER and a window is a window function, which
keeps every row and computes over the rows of each one's window, as
[Window functions](06-queries.md#window-functions) shows. `SUM(SALARY)
OVER (PARTITION BY DEPT)` is each row's department total. LISTAGG takes
a window only of no ORDER BY and no frame.

LISTAGG's separator is a literal, and its WITHIN GROUP's ORDER BY says
the order of the strings, each key by its collation. LISTAGG(DISTINCT
x, ...) joins each value once, the first of values alike in that order.
Strings joined longer than 32,767 characters are refused, SQLSTATE
22001, unless ON OVERFLOW TRUNCATE is written after the separator. Then
it joins as many as fit and ends with the separator and a filler, `...`
if none is written. WITH COUNT adds the number of strings left out in
parentheses, and WITHOUT COUNT does not.

A key of ARRAY_AGG's ORDER BY or of a WITHIN GROUP may be followed by
NULLS FIRST or NULLS LAST, as a key of a query's ORDER BY may.

FILTER (WHERE *condition*) after a set function's closing parenthesis
feeds it only the rows the condition is true of, so a row it is false or
unknown of is not counted and its argument is not computed. A group no
row of which passes gives COUNT 0 and the others null, and a null is
left out, with warning 01003, only of a row the filter takes. Each set
function has its own filter, so one query counts several subsets of the
same rows. FILTER comes before OVER, and over a window each frame's set
function takes the rows of the frame the filter is true of.

```sql
SELECT DEPT, COUNT(*) FILTER (WHERE SALARY > 6000) AS WELL_PAID,
    SUM(SALARY) FILTER (WHERE NAME <> 'Ada') AS NOT_ADA FROM STAFF GROUP BY DEPT ORDER BY DEPT;
SELECT ID, NAME, COUNT(*) FILTER (WHERE SALARY > 6000) OVER (ORDER BY ID) AS SO_FAR FROM STAFF ORDER BY ID;
```

```
 DEPT |            WELL_PAID |              NOT_ADA
------+----------------------+----------------------
 ENG  |                    2 |              8100.00
 OPS  |                    0 |              5400.00
 NULL |                    0 |                 NULL
(3 rows, with a warning: SQLSTATE 01003)
           ID | NAME                           |               SO_FAR
--------------+--------------------------------+----------------------
            1 | Ada                            |                    1
            2 | Grace                          |                    2
            3 | Edsger                         |                    2
            4 | Barbara                        |                    2
(4 rows)
```

A filter is of the rows its set function aggregates, so it holds no set
function, subquery, window function or nested window function, and no
outer reference. The filter of a set function of an enclosing query,
whose argument is an outer reference, names no column at all. Each is
refused, SQLCODE -103:

```sql
SELECT COUNT(*) FILTER (WHERE SALARY > AVG(SALARY)) FROM STAFF;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): a <filter clause> cannot contain a set function [ISO/IEC 9075-2:2023 §6.9 SR5] (at character 7)
```

RANK, DENSE_RANK, PERCENT_RANK and CUME_DIST of values, followed by
WITHIN GROUP and an ORDER BY of as many keys, are the hypothetical set
functions. Each gives what its window function would give a row of
those values, were it added to the group's rows and ordered among them
by the keys. RANK is one more than the rows before it, and DENSE_RANK
one more than the distinct rows before it, both of COUNT's type.
PERCENT_RANK is the rows before it over the group's rows, 0 of none, and
CUME_DIST the rows not after it, itself among them, over the group's
rows and it, both DOUBLE PRECISION. A row whose keys are all equal to
the values is a peer, not before. Of a group of no rows they are 1, 1, 0
and 1.

A null key is not left out but placed as its null ordering says, last
ascending and first descending unless NULLS FIRST or LAST is written.
Each value and its key are compared as of the type they have in common,
character strings by the collation they have together. The values are
not aggregated, so of a grouped query they name grouping columns and
columns functionally dependent on them, as the select list does, and of
one with no GROUP BY only a column of one value in all the rows. A `?`
value takes its key's type. A hypothetical set function takes FILTER
but no OVER.

```sql
SELECT DEPT, RANK(7500) WITHIN GROUP (ORDER BY SALARY DESC) AS PLACE,
    CUME_DIST(7500) WITHIN GROUP (ORDER BY SALARY) AS AT_MOST FROM STAFF GROUP BY DEPT ORDER BY DEPT;
SELECT RANK(7500) WITHIN GROUP (ORDER BY SALARY) OVER () FROM STAFF;
```

```
 DEPT |                PLACE |                  AT_MOST
------+----------------------+--------------------------
 ENG  |                    2 |     6.666666666666666E-1
 OPS  |                    1 |                      1E0
 NULL |                    2 |                     5E-1
(3 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): RANK WITHIN GROUP, a hypothetical set function, is over no window [ISO/IEC 9075-2:2023 §9.23 SR8c] (at character 7)
```

Barbara's null salary is placed first descending, before 7500, and
last ascending, after it.

PERCENTILE_CONT and PERCENTILE_DISC of a fraction p from 0 to 1,
followed by WITHIN GROUP and an ORDER BY of one key, are the inverse
distribution functions. They sort the key's values that are not null in
the key's direction. PERCENTILE_CONT takes the value p·(N−1) places
from the first of N, counted from 0. Where that falls between two
values it moves from the first toward the second by what is left over,
so the median of 7200 and 8100 is 7650. PERCENTILE_DISC takes the first
value of a share of the values up to it, its peers among them, that is
at least p, so it is always one of the values. The key is a number or
an interval. Of an exact or approximate key the result is DOUBLE
PRECISION, of a DECFLOAT key DECFLOAT(34), and of an interval the
interval's type.

A group of no values gives null, as does a null p. A p below 0 or above
1 is refused, SQLSTATE 22003, even of a group of no rows. Like a
hypothetical set function's values, p is not aggregated, and a `?` p is
NUMERIC(18,17). An exact p picks its places exactly, so a share equal to
p is at least it. Each takes FILTER, and OVER a window of a PARTITION BY
alone, whose every row has its partition's value, but not a window of
an ORDER BY or a frame. Over a window p may differ from row to row.

In a subquery, an inverse distribution function or hypothetical set
function whose sort keys name only columns of an enclosing query is
that query's, computed over its groups, and its p or values name that
query's grouping columns and columns functionally dependent on them. A key that names a column of the subquery too
keeps it in the subquery, where the enclosing row's columns are values.

```sql
SELECT DEPT, PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY SALARY) AS MEDIAN,
    PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY SALARY) AS MIDDLE FROM STAFF GROUP BY DEPT ORDER BY DEPT;
SELECT ID, NAME, PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY SALARY DESC) OVER (PARTITION BY DEPT) AS HIGH FROM STAFF ORDER BY ID;
SELECT PERCENTILE_CONT(1.5) WITHIN GROUP (ORDER BY SALARY) FROM STAFF;
SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY SALARY) OVER (ORDER BY ID) FROM STAFF;
```

```
 DEPT |                   MEDIAN |                   MIDDLE
------+--------------------------+--------------------------
 ENG  |                   7.65E3 |                    7.2E3
 OPS  |                    5.4E3 |                    5.4E3
 NULL |                     NULL |                     NULL
(3 rows, with a warning: SQLSTATE 01003)
           ID | NAME                           |                     HIGH
--------------+--------------------------------+--------------------------
            1 | Ada                            |                  7.875E3
            2 | Grace                          |                  7.875E3
            3 | Edsger                         |                    5.4E3
            4 | Barbara                        |                     NULL
(4 rows, with a warning: SQLSTATE 01003)
turborel: SQLCODE -301 (SQLSTATE 22003): the fraction of an inverse distribution function is from 0 to 1, not 1.5 [ISO/IEC 9075-2:2023 §10.9 GR10c]
turborel: SQLCODE -103 (SQLSTATE 2A000): PERCENTILE_DISC is over a window of no ordering and no frame [ISO/IEC 9075-2:2023 §9.23 SR8d i] (at character 7)
```

Engineering's upper quarter, sorted from the highest salary, is a
quarter of the way from 8100 to 7200.

ANY_VALUE gives a column that the grouping decides but does not name,
such as a department's name grouped by its number, with no need to
group by it too. Which value it gives of several is not defined.
Below, the four names fit, so the second query adds no filler.

```sql
SELECT DEPT, LISTAGG(NAME, ', ') WITHIN GROUP (ORDER BY SALARY DESC) AS BY_PAY,
    ANY_VALUE(NAME) AS SOMEONE FROM STAFF GROUP BY DEPT ORDER BY DEPT;
SELECT LISTAGG(NAME, ', ' ON OVERFLOW TRUNCATE '...' WITH COUNT) WITHIN GROUP (ORDER BY ID) FROM STAFF;
```

```
 DEPT | BY_PAY                                                       | SOMEONE
------+--------------------------------------------------------------+--------------------------------
 ENG  | Grace, Ada                                                   | Ada
 OPS  | Edsger                                                       | Edsger
 NULL | Barbara                                                      | Barbara
(3 rows)

--------------------------------------------------------------
 Ada, Grace, Edsger, Barbara
(1 row)
```

JSON_ARRAYAGG and JSON_OBJECTAGG make JSON text of a group's rows, as
[JSON_ARRAY and JSON_OBJECT](#json_object-and-json_array) do of their
operands, and keep the nulls the other set functions leave out.
JSON_ARRAYAGG takes ORDER BY within its parentheses, as ARRAY_AGG does,
and without one its order is that of the rows read. Each takes FILTER
and OVER, and the clauses of its constructor: `NULL ON NULL` or
`ABSENT ON NULL`, ABSENT ON NULL by default of JSON_ARRAYAGG and NULL ON
NULL of JSON_OBJECTAGG; of JSON_OBJECTAGG, `WITH UNIQUE KEYS`; FORMAT
JSON after the value; and `RETURNING type`. A null name is SQLSTATE
22004.

```sql
SELECT DEPT, JSON_ARRAYAGG(NAME ORDER BY SALARY DESC RETURNING VARCHAR(20)) AS BY_PAY,
    JSON_OBJECTAGG(NAME VALUE SALARY RETURNING VARCHAR(40)) AS PAY FROM STAFF GROUP BY DEPT ORDER BY DEPT;
SELECT JSON_ARRAYAGG(SALARY ORDER BY ID NULL ON NULL RETURNING VARCHAR(40)) FROM STAFF;
```

```
 DEPT | BY_PAY               | PAY
------+----------------------+------------------------------------------
 ENG  | ["Grace","Ada"]      | {"Ada":7200.00,"Grace":8100.00}
 OPS  | ["Edsger"]           | {"Edsger":5400.00}
 NULL | ["Barbara"]          | {"Barbara":null}
(3 rows)

------------------------------------------
 [7200.00,8100.00,5400.00,null]
(1 row)
```

## NEXT VALUE FOR

NEXT VALUE FOR *generator* takes the next value of a sequence generator
[CREATE SEQUENCE](statements/create-sequence.md) made, of its type. Each row
takes one value, however many times the row names the generator, and no
value is ever taken twice, even by a transaction rolled back.

```sql
CREATE SEQUENCE BADGE START WITH 100 INCREMENT BY 10;
SELECT NEXT VALUE FOR BADGE, NEXT VALUE FOR BADGE + 1, NAME FROM STAFF WHERE DEPT = 'ENG';
```

```
OK
                      |                      | NAME
----------------------+----------------------+--------------------------------
                  100 |                  101 | Ada
                  110 |                  111 | Grace
(2 rows)
```

It is written in a select list of a cursor's query, a SELECT INTO or an
INSERT's query, in an INSERT's VALUES, or in an UPDATE's SET, and in no
CASE, condition, set function, grouped or DISTINCT query or subquery, where
a row might take a value it never shows.

## Arrays and rows

An array is made of its elements, `ARRAY[1, 2, 3]`, or of the rows of a
query of one column, `ARRAY(SELECT ID FROM STAFF ORDER BY ID)`, in the
order its ORDER BY gives. `ARRAY[]` is an array of no elements, written
where its type is known, as an INSERT's value or in a CAST. A row is
made of its fields, `ROW(1, 'Ada')`, named F1, F2 and so on.

| Expression | Value |
|---|---|
| a[i] | element i of the array a, from 1; null if a or i is null |
| CARDINALITY(a) | the elements of a |
| ARRAY_MAX_CARDINALITY(a) | the maximum cardinality of a's type |
| TRIM_ARRAY(a, n) | a without its last n elements |
| a \|\| b | the elements of a, then those of b |
| r.f, (r).f | the field f of the row r; null if r is null |

An element reference past the cardinality, or TRIM_ARRAY of more
elements than there are, is refused, SQLSTATE 2202E. A column's field is
written after the column, `T.C.F` or `C.F`, as far as its rows nest.

```sql
SELECT ID, ARRAY[ID, ID * 10] AS PAIR, ROW(ID, NAME) AS PERSON FROM STAFF WHERE ID < 3 ORDER BY ID;
SELECT ARRAY_AGG(NAME ORDER BY SALARY DESC) AS BY_PAY FROM STAFF WHERE SALARY IS NOT NULL;
VALUES (CARDINALITY(ARRAY[1, 2, 3]), TRIM_ARRAY(ARRAY[1, 2, 3], 1), ARRAY[1] || ARRAY[2, 3], (ROW(1, 'Ada')).F2);
VALUES (ARRAY[1, 2, 3])[4];
```

```
           ID | PAIR                                                | PERSON
--------------+-----------------------------------------------------+-----------------------------------------------------
            1 | ARRAY[1, 10]                                        | ROW(1, 'Ada')
            2 | ARRAY[2, 20]                                        | ROW(2, 'Grace')
(2 rows)
 BY_PAY
--------------------------------------------------------------
 ARRAY['Grace', 'Ada', 'Edsger']
(1 row)
              |                        |                        |
--------------+------------------------+------------------------+-----
            3 | ARRAY[1, 2]            | ARRAY[1, 2, 3]         | Ada
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2202E): no element 4 is in an array of 3 elements [ISO/IEC 9075-2:2023 §6.25 GR2b]
```

## Multisets

A multiset is made of its elements, `MULTISET[1, 2, 2]`, or of the rows
of a query of one column, `MULTISET(SELECT DEPT FROM STAFF)`. `TABLE(query)`
makes a multiset of a row of each of a query's rows. Neither query has
an ORDER BY. `MULTISET[]` is a multiset of no elements, written where its
type is known.

| Expression | Value |
|---|---|
| m MULTISET UNION [ALL \| DISTINCT] n | the elements of both; DISTINCT one of each |
| m MULTISET EXCEPT [ALL \| DISTINCT] n | the elements of m less as many of n; DISTINCT those n lacks, one of each |
| m MULTISET INTERSECT [ALL \| DISTINCT] n | as many of each element as the one with fewer has; DISTINCT one of each |
| SET(m) | one element of each, nulls counting as alike |
| ELEMENT(m) | the one element of m; null if m is null or empty |
| CARDINALITY(m) | the elements of m |

Each operator is ALL if neither is written, and null if an operand is.
MULTISET INTERSECT binds tighter than UNION and EXCEPT, which are read
left to right. ELEMENT of a multiset of more than one element is refused,
SQLSTATE 21000. COLLECT, FUSION and INTERSECTION, in [Set
functions](#set-functions), gather multisets of a group's rows.

```sql
VALUES (MULTISET[1, 1, 2] MULTISET UNION MULTISET[2, 3], MULTISET[1, 1, 2] MULTISET UNION DISTINCT MULTISET[2, 3]);
VALUES (MULTISET[1, 1, 2] MULTISET EXCEPT MULTISET[1], MULTISET[1, 1, 2] MULTISET INTERSECT MULTISET[1, 2, 2]);
VALUES (SET(MULTISET[1, 1, 2]), ELEMENT(MULTISET['only']), CARDINALITY(MULTISET[3, 3]));
SELECT DEPT, COLLECT(NAME) AS WHO FROM STAFF WHERE DEPT IS NOT NULL GROUP BY DEPT ORDER BY DEPT;
VALUES MULTISET(SELECT DEPT FROM STAFF WHERE DEPT IS NOT NULL);
VALUES ELEMENT(MULTISET[1, 2]);
```

```
                                                              |
--------------------------------------------------------------+--------------------------------------------------------------
 MULTISET[1, 1, 2, 2, 3]                                      | MULTISET[1, 2, 3]
(1 row)
                                                              |
--------------------------------------------------------------+--------------------------------------------------------------
 MULTISET[1, 2]                                               | MULTISET[1, 2]
(1 row)
                                                              |      |
--------------------------------------------------------------+------+--------------
 MULTISET[1, 2]                                               | only |            2
(1 row)
 DEPT | WHO
------+--------------------------------------------------------------
 ENG  | MULTISET['Ada', 'Grace']
 OPS  | MULTISET['Edsger']
(2 rows)

--------------------------------------------------------------
 MULTISET['ENG', 'ENG', 'OPS']
(1 row)
turborel: SQLCODE -811 (SQLSTATE 21000): ELEMENT of a multiset of 2 elements [ISO/IEC 9075-2:2023 §6.26 SR1, §7.11 GR1a]
```

## Structured values and references

A value of a structured type ([CREATE TYPE](statements/create-type.md))
is made by NEW, `NEW t()` of its attributes' defaults or `NEW t(args)`
by a constructor method. `v.a` observes an attribute and `v.a(x)` gives
a copy with it changed, so a value is built a mutator at a time. A
method is invoked of a value, `v.m(args)`, and runs by the value's most
specific type. A static method is invoked of its type, `t::m(args)`.

A reference identifies a row of a typed table. `r -> a` reads an
attribute of the row, `r -> m(args)` invokes a method of it, and
`DEREF(r)` gives the row as a value of its type, each null if the row
is gone. `v.SPECIFICTYPE` names a value's most specific type, and
`TREAT(v AS t)` takes it as a value of its subtype `t`.

```sql
CREATE TYPE ITEM AS (LABEL VARCHAR(10), GRAMS INTEGER) NOT FINAL REF IS SYSTEM GENERATED
    METHOD KILOS () RETURNS DECIMAL(9,3) CONTAINS SQL;
CREATE METHOD KILOS () RETURNS DECIMAL(9,3) FOR ITEM RETURN SELF.GRAMS / 1000.0;
CREATE TYPE GIFT UNDER ITEM AS (WRAP VARCHAR(10)) NOT FINAL;
CREATE TABLE ITEMS OF ITEM (REF IS ITEM_ID SYSTEM GENERATED);
CREATE TABLE GIFTS OF GIFT UNDER ITEMS;
INSERT INTO ITEMS (LABEL, GRAMS) VALUES ('LAMP', 1500);
INSERT INTO GIFTS (LABEL, GRAMS, WRAP) VALUES ('BOOK', 400, 'RED');
CREATE TABLE ORDERS (N INTEGER, WHAT REF(ITEM) SCOPE ITEMS);
INSERT INTO ORDERS SELECT CASE LABEL WHEN 'LAMP' THEN 1 ELSE 2 END, ITEM_ID FROM ITEMS;
SELECT N, WHAT -> LABEL, WHAT -> KILOS() FROM ORDERS ORDER BY N;
SELECT N, (DEREF(WHAT)).SPECIFICTYPE FROM ORDERS ORDER BY N;
SELECT NEW ITEM().LABEL('CUP').GRAMS(250).KILOS() FROM (VALUES (1)) AS T (X);
```

```
OK
OK
OK
OK
OK
OK
OK
OK
OK
            N |            |
--------------+------------+-------------
            1 | LAMP       |       1.500
            2 | BOOK       |       0.400
(2 rows)
            N |
--------------+--------------------------------------------------------------
            1 | "TURBOREL"."ANN"."ITEM"
            2 | "TURBOREL"."ANN"."GIFT"
(2 rows)

-------------
       0.250
(1 row)
```

## SQL/JSON

JSON_VALUE and JSON_QUERY take a value out of JSON text, a character
string, by an SQL/JSON path expression;
[JSON_EXISTS](05-predicates.md#json_exists) tests whether the path
finds anything. Each is written as

```
JSON_VALUE(text [FORMAT JSON], 'path' [PASSING value [FORMAT JSON] AS name, ...]
           [RETURNING type] [behavior ON EMPTY] [behavior ON ERROR])
JSON_QUERY(text [FORMAT JSON], 'path' [PASSING ...] [RETURNING type [FORMAT JSON]]
           [WITHOUT [ARRAY] | WITH [CONDITIONAL | UNCONDITIONAL] [ARRAY] WRAPPER]
           [KEEP | OMIT QUOTES [ON SCALAR STRING]] [behavior ON EMPTY] [behavior ON ERROR])
```

The path is a character string literal, read as the statement is
prepared, so a path that is none is refused then.

| Function | Value |
|---|---|
| JSON_VALUE | the one scalar the path finds, a string, number, truth value or datetime, cast to the RETURNING type: a character string, numeric, BOOLEAN or datetime type, CHARACTER VARYING(32767) if none is written |
| JSON_QUERY | the one item the path finds, an array or object too, written as JSON text of the RETURNING type, a character string type, CHARACTER VARYING(32767) if none is written |

Either is null if the text is. A behavior ON EMPTY says what the value
is if the path finds nothing, and one ON ERROR what it is in place of a
data exception, as text that is no JSON, two items found, or an array
for JSON_VALUE. Each is NULL if not written. JSON_VALUE takes ERROR,
NULL or DEFAULT and a value expression, cast to the RETURNING type;
JSON_QUERY takes ERROR, NULL, EMPTY ARRAY or EMPTY OBJECT. ERROR ON
EMPTY makes an empty result an error that ON ERROR then replaces, so
ERROR ON EMPTY alone is null of no item. Only a data exception, an
SQLSTATE of class 22, is replaced: an error of a limit, SQLCODE -900, is
raised whatever ON ERROR says.

```sql
CREATE TABLE PURCHASES (ID INTEGER PRIMARY KEY, DOC VARCHAR(200));
INSERT INTO PURCHASES VALUES (1, '{"buyer":"Ada","items":[{"sku":"A1","qty":2},{"sku":"B7","qty":1}]}'),
                             (2, '{"buyer":"Grace","items":[],"note":null}');
SELECT ID, JSON_VALUE(DOC, 'lax $.buyer' RETURNING VARCHAR(10)), JSON_VALUE(DOC, 'lax $.items.size()' RETURNING INTEGER),
       JSON_VALUE(DOC, 'strict $.items[0].qty' RETURNING INTEGER DEFAULT 0 ON ERROR)
  FROM PURCHASES ORDER BY ID;
```

```
OK
OK
           ID |            |              |
--------------+------------+--------------+--------------
            1 | Ada        |            2 |            2
            2 | Grace      |            0 |            0
(2 rows)
```

JSON_QUERY without a wrapper finds one item, and WITH ARRAY WRAPPER
puts all it finds in an array. WITH CONDITIONAL ARRAY WRAPPER does so
unless the path finds one array or object, so a lone string or number is
wrapped. OMIT QUOTES gives a string found alone as it is, where KEEP
QUOTES, the default, writes it as JSON, in quotation marks. A wrapper
WITH and a QUOTES clause are not written together.

```sql
SELECT ID, JSON_QUERY(DOC, 'lax $.items[*].sku' RETURNING VARCHAR(20) WITH ARRAY WRAPPER),
       JSON_QUERY(DOC, 'lax $.items[0]' RETURNING VARCHAR(30) EMPTY OBJECT ON EMPTY),
       JSON_QUERY(DOC, 'lax $.buyer' RETURNING VARCHAR(10) OMIT QUOTES)
  FROM PURCHASES ORDER BY ID;
```

```
           ID |                      |                                |
--------------+----------------------+--------------------------------+------------
            1 | ["A1","B7"]          | {"sku":"A1","qty":2}           | Ada
            2 | NULL                 | {}                             | Grace
(2 rows)
```

### The path language

A path begins with its mode, `lax` or `strict`, then `$`, the text, and
its accessors: `.name` or `."name"` a member of an object, `.*` every
member's value, `[n]`, `[n to m]` and `[last]` elements of an array,
counted from 0, `[*]` every element, and the item methods. `.type()`
names an item's kind and `.size()` counts an array's elements;
`.double()`, `.ceiling()`, `.floor()` and `.abs()` compute; `.bigint()`,
`.integer()`, `.decimal(p, s)`, `.number()`, `.boolean()`, `.string()`,
`.date()`, `.time(p)`, `.time_tz(p)`, `.timestamp(p)` and
`.timestamp_tz(p)` cast an item as CAST does, the precisions optional;
`.datetime()` reads a string as the date, time or timestamp it is, and
`.datetime("template")` as the template reads it, as in
[CAST](#cast)'s FORMAT: a TIMESTAMP if the template has fields of the
date and the time, a DATE or a TIME if of one, WITH TIME ZONE if it has
TZH, a string it does not read being SQLSTATE 22031; and
`.keyvalue()` gives an object's members, each as an object of its
`key`, its `value` and an `id`. A number may be written in hexadecimal,
`0x1F`. A
filter, `? (predicate)`, keeps the items its predicate is true of, `@`
being the item tested: `==`, `!=`, `<>`, `<`, `<=`, `>` and `>=`
compare, `exists (path)`, `starts with` and `like_regex` test, and
`&&`, `||`, `!` and `(...) is unknown` combine. `+`, `-`, `*`, `/` and
`%` compute with numbers. Unlike SQL, the language is case-sensitive:
`$.Buyer` is not `$.buyer`, and `true`, `null` and the methods are lower
case.

The casting methods take what CAST takes, and an item of another kind
is an exception ON ERROR replaces, as `.date()` of a string that is no
date:

```sql
VALUES (JSON_VALUE('{"n":"12.345"}', 'lax $.n.decimal(4,2)' RETURNING DECIMAL(4,2)), JSON_VALUE('"2024-02-29"', 'lax $.date()' RETURNING DATE),
        JSON_VALUE('{}', 'lax 0x1F' RETURNING INTEGER));
VALUES JSON_VALUE('"x"', 'lax $.date()' ERROR ON ERROR);
```

```
        |            |
--------+------------+--------------
  12.34 | 2024-02-29 |           31
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22018): 'x' is no DATE literal's string [ISO/IEC 9075:1992 §6.10 GR9a ii, GR10a ii, GR11a ii]
```

Lax mode takes an array for its elements where an object or a single
item is wanted, and a single item for an array, and a member or element
that is not there for nothing. Strict mode takes them as errors, which
ON ERROR then replaces.

```sql
VALUES (JSON_VALUE('{"a":[1,2]}', 'lax $.a[*]' RETURNING INTEGER), JSON_VALUE('{"a":[1,2]}', 'lax $.a[last]' RETURNING INTEGER),
        JSON_VALUE('{"a":[1,2]}', 'lax $.a.type()' RETURNING VARCHAR(10)), JSON_VALUE('[{"a":3}]', 'lax $.a' RETURNING INTEGER),
        JSON_VALUE('[{"a":3}]', 'strict $.a' RETURNING INTEGER));
VALUES JSON_VALUE('[1,2]', 'lax $[*]' RETURNING INTEGER ERROR ON ERROR);
VALUES JSON_VALUE('{}', 'strict $.a' RETURNING INTEGER ERROR ON ERROR);
VALUES JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER ERROR ON EMPTY);
```

```
              |              |            |              |
--------------+--------------+------------+--------------+--------------
         NULL |            2 | array      |            3 |         NULL
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22034): the SQL/JSON path expression gives more than one SQL/JSON item [ISO/IEC 9075-2:2023 §9.48 GR4a]
turborel: SQLCODE -301 (SQLSTATE 2203A): an SQL/JSON item is not an object with a member of the key a [ISO/IEC 9075-2:2023 §9.46 GR11g ii 1 B II]

--------------
         NULL
(1 row)
```

```sql
VALUES (JSON_VALUE('"29.02.2024"', 'lax $.datetime("DD.MM.YYYY")' RETURNING DATE),
        JSON_VALUE('"9:05 P.M."', 'lax $.datetime("HH12:MI A.M.").type()' RETURNING VARCHAR(30)));
```

```
            |
------------+--------------------------------
 2024-02-29 | time without time zone
(1 row)
```

PASSING gives the path variables: `PASSING value AS name` is `$name` in
the path, a string, number, truth value or datetime as it is, and with
FORMAT JSON a string's JSON text, parsed. The name is the identifier's
value, as SQL reads one: a regular identifier is upper case, so
`PASSING 2 AS n` is `$N`, and a delimited one is as written, so
`PASSING 'B' AS "p"` is `$p`. A variable the path names and PASSING
does not is an error as the statement is prepared.

```sql
SELECT ID, JSON_VALUE(DOC, 'lax $.items[*] ? (@.qty >= $N).sku' PASSING 2 AS N RETURNING VARCHAR(5)),
       JSON_VALUE(DOC, 'lax $.items[*] ? (@.sku starts with $p).qty' PASSING 'B' AS "p" RETURNING INTEGER)
  FROM PURCHASES ORDER BY ID;
VALUES JSON_VALUE('{}', 'lax $n' PASSING 2 AS n);
```

```
           ID |       |
--------------+-------+--------------
            1 | A1    |            1
            2 | NULL  |         NULL
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): the named variable $n of the SQL/JSON path expression is passed no value [ISO/IEC 9075-2:2023 §9.46 SR4] (at character 24)
```

A cast that cuts a string to the RETURNING type completes with the
warning 01004, as CAST does. A binary string's JSON text, FORMAT JSON
ENCODING, is valid SQL turborel does not process: SQLCODE -900.
[Appendix A](../appendix/a-sqlstate.md) lists the SQLSTATEs of
SQL/JSON, 22032 to 2203G, and [Limits](../appendix/d-limits.md) how deep
JSON text and a path may nest. The functions' names and the words of
their clauses are not reserved, so a column may be named any of them.

### JSON_OBJECT and JSON_ARRAY

JSON_OBJECT and JSON_ARRAY make JSON text of their operands:

```
JSON_OBJECT([[KEY] name VALUE value [FORMAT JSON] | name : value [FORMAT JSON], ...
            [NULL ON NULL | ABSENT ON NULL] [WITH UNIQUE [KEYS] | WITHOUT UNIQUE [KEYS]]]
            [RETURNING type [FORMAT JSON]])
JSON_ARRAY([value [FORMAT JSON], ... [NULL ON NULL | ABSENT ON NULL]] [RETURNING type [FORMAT JSON]])
JSON_ARRAY(query [FORMAT JSON] [NULL ON NULL | ABSENT ON NULL] [RETURNING type [FORMAT JSON]])
```

A name is a character string, and a null one is SQLSTATE 22004. A value
that is a string, a number or a truth value is kept, one of another
type, as a date, written as a string, and a value FORMAT JSON is the
JSON text it holds, parsed. A value that is itself a constructor or a
JSON_QUERY is FORMAT JSON of itself, so constructors nest. NULL ON NULL
keeps a null value as JSON's null, ABSENT ON NULL leaves it out;
JSON_OBJECT keeps it by default, JSON_ARRAY leaves it out. WITH UNIQUE
KEYS makes two members of one name SQLSTATE 22030. JSON_ARRAY of a query
is an array of its one column's values, in the order its ORDER BY gives,
`[]` of no rows; a lone subquery in parentheses is so too. Each is
CHARACTER VARYING(32767) without RETURNING, and text longer than its
type is SQLSTATE 22001.

```sql
SELECT JSON_OBJECT('id' VALUE ID, 'buyer' : JSON_VALUE(DOC, 'lax $.buyer'), 'note' VALUE JSON_QUERY(DOC, 'lax $.note')
                   ABSENT ON NULL RETURNING VARCHAR(40))
  FROM PURCHASES ORDER BY ID;
VALUES (JSON_ARRAY(1, 'two', CAST(NULL AS INTEGER), JSON_ARRAY(3, 4) RETURNING VARCHAR(20)),
        JSON_ARRAY(SELECT ID FROM PURCHASES ORDER BY ID DESC RETURNING VARCHAR(10)));
VALUES JSON_OBJECT('a' VALUE 1, 'a' VALUE 2 WITH UNIQUE KEYS);
```

```

------------------------------------------
 {"id":1,"buyer":"Ada"}
 {"id":2,"buyer":"Grace"}
(2 rows)
                      |
----------------------+------------
 [1,"two",[3,4]]      | [2,1]
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22030): two members of a JSON object WITH UNIQUE KEYS have the key "a" [ISO/IEC 9075-2:2023 §6.34 GR2d ii 2]
```

Grace's note is JSON's null, which JSON_QUERY gives as the text `null`
and JSON_OBJECT, of it FORMAT JSON, leaves out ABSENT ON NULL. In
JSON_ARRAY of a query whose FROM clause ends in a table of no
correlation name, FORMAT is read as that table's correlation name:
write `JSON_ARRAY(SELECT J FROM T AS T FORMAT JSON)`, or the query in
parentheses. The standard counts JSON_VALUE, JSON_QUERY and JSON_EXISTS
as possibly non-deterministic, so a CHECK constraint or a generated
column holds none of them, SQLCODE -103; the constructors and [IS
JSON](05-predicates.md#is-json) may be there.

## Conditions as values

A [condition](05-predicates.md), such as `SALARY > 6000` or `DEPT IS
NULL OR SALARY < 6000`, is a value of type BOOLEAN wherever a value is
written: in a select list, a VALUES row, an UPDATE's SET, a CASE result,
a CAST operand, a set function's argument, or in parentheses. Its value
is TRUE, FALSE, or null when it is unknown. In a comparison, a condition
of its own is written in parentheses, `(A = 1) = (B = 2)`.

## CAST

CAST(*value* AS *type*) gives a value in another type, or a domain's:
[Data Types](02-data-types.md) shows which types CAST takes to which.

CAST(*value* AS *type* FORMAT '*template*') writes a datetime as a
character string the template describes, or reads such a string back
as a datetime. The template is a string literal of these fields, in
upper case, and the delimiters `- . / , ' ; :` and space.

| Field | Means |
|---|---|
| YYYY, YYY, YY, Y | the year's last digits; read, the current year's first digits before them |
| RRRR, RR | read as YYYY and YY |
| MM, DD, DDD | the month, the day of the month, the day of the year |
| HH24, HH12 or HH, A.M. or P.M. | the hour on a 24-hour clock, or on a 12-hour one beside A.M. or P.M. |
| MI, SS, SSSSS | the minute, the second, the second of the day |
| FF1 to FF9 | so many digits of the second's fraction, truncated |
| TZH, TZM | the time zone's hour, signed, and minute |

```sql
VALUES (CAST(DATE '2024-03-05' AS VARCHAR(10) FORMAT 'DD/MM/YYYY'), CAST(TIMESTAMP '2024-03-05 14:07:09' AS VARCHAR(10) FORMAT 'HH12:MI P.M.'),
        CAST('5/3/2024' AS DATE FORMAT 'DD/MM/YYYY'), CAST('2024 065 +05:30' AS TIMESTAMP(0) WITH TIME ZONE FORMAT 'YYYY DDD TZH:TZM'));
```

```
            |            |            |
------------+------------+------------+---------------------------
 05/03/2024 | 02:07 P.M. | 2024-03-05 | 2024-03-05 00:00:00+05:30
(1 row)
```

A field between delimiters may be written with fewer digits, so
`'5/3/2024'` reads as above, and one beside another field takes all its
digits. A field not written is read as the current year or month, day 1,
or 0. The string is read whole, its spaces kept, and a string the
template does not describe, or that writes no date or time, is SQLSTATE
22007. A datetime without time zone is written and read in the session's
time zone. A template that is not valid for the types, or a string type
shorter than the template's least length, is refused as the statement
is prepared (SQLCODE -103).

The standard: ISO/IEC 9075:1992 §6.2–§6.15 and §7.11; X3.135-1986 §5.6–§5.9;
ISO/IEC 9075-2:2023 §6.29, §6.46 and §10.9 for truth values, §6.31–§6.33
for binary strings, §6.13 and §9.50–§9.52 for FORMAT, §6.14 and §9.35 for
NEXT VALUE FOR.
