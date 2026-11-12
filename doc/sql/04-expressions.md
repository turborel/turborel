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
  identifier; SESSION_USER and SYSTEM_USER, which are the same here, a
  session having no other;
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

## String expressions

`||` joins two strings, or two binary strings; of CHARACTER VARYING, or
BINARY VARYING, if either is. Binary strings joined past 32,767 octets
keep their first ones if the rest are X'00', else are refused (SQLSTATE
22001). The string functions:

| Function | Value |
|---|---|
| SUBSTRING(s FROM start [FOR length]) | the characters of s from start, counting from 1, length of them or the rest; of a binary string, its octets |
| UPPER(s), LOWER(s) | s in upper or lower case |
| TRIM([[LEADING \| TRAILING \| BOTH] [c] FROM] s) | s without the character c, a space if not written, at its start, end or both, the default |

A function of a CHARACTER VARYING string is CHARACTER VARYING, and
SUBSTRING of a binary string BINARY VARYING. UPPER, LOWER and TRIM take
character strings alone.

```sql
VALUES ('Ada' || ' ' || 'Lovelace', SUBSTRING('Lovelace' FROM 1 FOR 4), UPPER('ada'),
        POSITION('ve' IN 'Lovelace'), CHAR_LENGTH('Ada'));
VALUES ('[' || TRIM('  x  ') || ']', TRIM(LEADING '0' FROM '007'), OCTET_LENGTH('é'), BIT_LENGTH('A'));
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

CONVERT and TRANSLATE, of character sets other than SQL_TEXT, are valid
SQL turborel does not process: SQLCODE -900.

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
| ANY(x), SOME(x) | of a BOOLEAN x, TRUE if an x is TRUE, else FALSE |

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
refused only if the mean does not fit. A SUM of DECFLOAT values is
rounded to 34 digits after each row, so of values whose total needs
more its last digit may depend on that order.

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
even of no rows, so each column it selects is in a set function. A set function is not in a
WHERE clause, which is applied to rows one by one:

```sql
SELECT NAME, COUNT(*) FROM STAFF;
SELECT ID FROM STAFF WHERE SALARY > AVG(SALARY);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): when the select list has a set function, every column reference must be inside one (found STAFF.NAME) [X3.135-1986 §5.25 SR7] (at character 7)
turborel: SQLCODE -103 (SQLSTATE 2A000): a <set function specification> is in a <where clause> only in a <having clause>'s <subquery>, of an outer reference [X3.135-1986 §5.21 SR3] (at character 36)
```

The mean is found by a subquery: `WHERE SALARY > (SELECT AVG(SALARY)
FROM STAFF)`.

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

The standard: ISO/IEC 9075:1992 §6.2–§6.15 and §7.11; X3.135-1986 §5.6–§5.9;
ISO/IEC 9075-2:2023 §6.29, §6.46 and §10.9 for truth values, §6.31–§6.33
for binary strings.
