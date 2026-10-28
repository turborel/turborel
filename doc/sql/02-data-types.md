# Data Types

[Contents](README.md)

Every value is of a data type, and every column, parameter and
expression has one. A column may also be of a [domain](statements/create-domain.md),
which is a data type with a default and constraints, or *(extension)* of
a type a plug-in registers, written by its name and, in parentheses, one
or two unsigned integers it takes.

```
<data type> ::=
      <character string type> | <national character string type>
    | <bit string type>
    | <exact numeric type> | <approximate numeric type>
    | <datetime type> | <interval type>
```

| Type | Holds | Written |
|---|---|---|
| CHARACTER(n) | n characters, padded with spaces | CHAR(n); CHAR is CHAR(1) |
| CHARACTER VARYING(n) | up to n characters | CHAR VARYING(n), VARCHAR(n) |
| NATIONAL CHARACTER(n) | as CHARACTER | NCHAR(n), NATIONAL CHAR(n), and VARYING |
| BIT(n) | n bits | BIT is BIT(1) |
| BIT VARYING(n) | up to n bits | |
| NUMERIC(p, s), DECIMAL(p, s) | exact numbers of p digits, s after the point | DEC(p, s); (p) is (p, 0); no precision is 18 |
| INTEGER | NUMERIC(10, 0) | INT |
| SMALLINT | NUMERIC(5, 0) | |
| REAL | approximate numbers of 24 binary digits | FLOAT(p) for p up to 24 |
| DOUBLE PRECISION | approximate numbers of 53 binary digits | FLOAT, FLOAT(p) for p from 25 to 53 |
| DATE | year, month, day | |
| TIME(p) | hour, minute, second of p fraction digits | TIME is TIME(0); WITH TIME ZONE |
| TIMESTAMP(p) | a date and a time | TIMESTAMP is TIMESTAMP(6); WITH TIME ZONE |
| INTERVAL q | a span of time, of the fields its qualifier names | INTERVAL '3' DAY |

The longest string is 32,767 characters, the longest bit string 262,136
bits, and an exact number holds at most 18 digits. A type past a limit
is valid SQL turborel does not process: SQLCODE -900, SQLSTATE 0A000.
The [Limits](../appendix/d-limits.md) appendix lists them all.

## Character strings

A character string is a sequence of Unicode characters. CHARACTER(n)
holds exactly n: a shorter value is padded with spaces as it is stored.
CHARACTER VARYING(n) holds the value as written, up to n characters;
its length must be written. Strings are compared as if the shorter were
padded with spaces, so `'ab'` equals `'ab   '`, and ordered by Unicode
scalar value.

A value longer than its column is refused, unless all it has past the
length is spaces, which are dropped:

```sql
CREATE TABLE STR (F CHAR(5), V VARCHAR(5));
INSERT INTO STR VALUES ('ab', 'ab');
SELECT '[' || F || ']' AS FIXED, '[' || V || ']' AS VAR, CHAR_LENGTH(F) AS FL, CHAR_LENGTH(V) AS VL FROM STR;
SELECT COUNT(*) FROM STR WHERE F = V;
INSERT INTO STR VALUES ('abcdef', 'x');
INSERT INTO STR VALUES ('abcd  ', 'abcde   ');
```

```
OK
OK
 FIXED   | VAR     |                   FL |                   VL
---------+---------+----------------------+----------------------
 [ab   ] | [ab]    |                    5 |                    2
(1 row)

----------------------
                    1
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22001): a value of 6 characters, not spaces beyond 5, does not fit CHARACTER(5) [ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3b ii]
OK
```

NATIONAL CHARACTER is CHARACTER: there is one character set, SQL_TEXT,
which holds every Unicode character, so the national one is the same.
The Information Schema shows such a column as CHARACTER.

## Bit strings

A bit string is a sequence of bits, written `B'1010'` in binary or
`X'FF'` in hexadecimal. A value of BIT(n) has exactly n bits: one of
another length is refused. BIT VARYING(n) holds up to n. Bit strings are
compared bit by bit; BIT_LENGTH gives a bit string's length.

```sql
CREATE TABLE BT (B BIT(8), V BIT VARYING(16));
INSERT INTO BT VALUES (B'10100000', X'FF');
SELECT B, V, BIT_LENGTH(V), B || V FROM BT;
INSERT INTO BT (B) VALUES (B'101');
```

```
OK
OK
 B        | V                |                      |
----------+------------------+----------------------+--------------------------
 10100000 | 11111111         |                    8 | 1010000011111111
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22026): a bit string of 3 bits is not of BIT(8)'s length [ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3h]
```

A bit string is not a number: CAST takes one to and from character
strings, whose UTF-8 bytes are its bits, and not to or from numbers.

## Numbers

**Exact numbers** are decimal: NUMERIC(p, s) and DECIMAL(p, s) hold p
digits, s of them after the decimal point, and are the same type.
INTEGER is NUMERIC(10, 0), SMALLINT NUMERIC(5, 0). A value with more
fraction digits than its column's scale is truncated to fit; one with
more digits before the point than the column holds is refused.

Arithmetic on exact numbers is exact, to 18 digits. A sum or a product
has the precision 18; a quotient is exact at the largest scale that
fits in 18 digits, so 7 / 2 is 3.5, and 1 / 3.0 is
0.333333333333333333. A result past 18 digits is refused.

**Approximate numbers** are binary floating point: REAL of 24 binary
digits, DOUBLE PRECISION of 53. FLOAT(p) is REAL for p up to 24, else
DOUBLE PRECISION; FLOAT is DOUBLE PRECISION. They are shown in E
notation. A REAL keeps only what 24 bits hold, as 0.1 shows:

```sql
CREATE TABLE NUM (I INTEGER, S SMALLINT, D DECIMAL(5,2), R REAL, P DOUBLE PRECISION);
INSERT INTO NUM VALUES (2147483647, 32767, 123.456, 0.1, 0.1);
SELECT * FROM NUM;
INSERT INTO NUM (D) VALUES (1234.5);
SELECT 7 / 2, 7.0 / 2, 1 / 3.0, 7E0 / 2 FROM NUM;
SELECT 999999999999999999 + 1 FROM NUM;
```

```
OK
OK
            I |       S |       D |                        R |                        P
--------------+---------+---------+--------------------------+--------------------------
   2147483647 |   32767 |  123.45 |    1.0000000149011612E-1 |                     1E-1
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22003): 1234.50 does not fit DECIMAL(5,2) without losing leading digits [ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3k]
                      |                      |                      |
----------------------+----------------------+----------------------+--------------------------
                  3.5 |                  3.5 | 0.333333333333333333 |                    3.5E0
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22003): the exact numeric result needs more than the implementor-defined 18 digits [X3.135-1986 §5.9 GR6]
```

INTEGER holds 2147483647, and more: it is NUMERIC(10, 0), of ten
digits. A type is shown, in messages and in the Information Schema, by
the name it was written with, INTEGER or DECIMAL(5,2); a type computed,
of a sum or a literal, as NUMERIC or FLOAT. The name is no part of the
type: INTEGER and NUMERIC(10, 0) are one type, so a foreign key of one
references the other.

## Datetimes

DATE holds a date of the years 1 to 9999; TIME(p) a time of day, its
seconds of p fraction digits, at most 6; TIMESTAMP(p) a date and a time.
WITH TIME ZONE adds a time zone displacement, from -12:59 to +13:00.
Literals are written `DATE '2026-10-28'`, `TIME '13:45:30.125'`,
`TIMESTAMP '2026-10-28 13:45:30'`, a displacement after the time if
written, `+02:00`.

A datetime is kept as an instant, in UTC. One with a time zone keeps its
displacement and is shown in it. One without is taken in the session's
time zone ([SET TIME ZONE](statements/set-time-zone.md)) as it is
written, and shown in the session's zone as it is read: so after SET
TIME ZONE, it shows the same instant in the new zone. Datetimes are
compared as instants. CURRENT_DATE, CURRENT_TIME and CURRENT_TIMESTAMP
give the instant the statement began, the same throughout it. EXTRACT
gives a field: YEAR, MONTH, DAY, HOUR, MINUTE, SECOND, and of a datetime
with a time zone TIMEZONE_HOUR and TIMEZONE_MINUTE.

```sql
CREATE TABLE EV (D DATE, T TIME(2), TS TIMESTAMP, TZ TIMESTAMP WITH TIME ZONE);
INSERT INTO EV VALUES (DATE '2026-10-28', TIME '13:45:30.125',
    TIMESTAMP '2026-10-28 13:45:30.123456', TIMESTAMP '2026-10-28 13:45:30+02:00');
SELECT * FROM EV;
SET TIME ZONE INTERVAL '+05:30' HOUR TO MINUTE;
SELECT TZ, TS FROM EV;
INSERT INTO EV (D) VALUES (DATE '2026-02-30');
```

```
OK
OK
 D          | T           | TS                         | TZ
------------+-------------+----------------------------+----------------------------------
 2026-10-28 | 13:45:30.12 | 2026-10-28 13:45:30.123456 | 2026-10-28 13:45:30.000000+02:00
(1 row)
OK
 TZ                               | TS
----------------------------------+----------------------------
 2026-10-28 13:45:30.000000+02:00 | 2026-10-28 19:15:30.123456
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): DATE '2026-02-30' is no valid <datetime literal> [ISO/IEC 9075:1992 §5.3 SR21, SR22, SR25] (at character 27)
```

TIME(2) kept two of the three fraction digits written, the third
dropped. The session began at +00:00, so TS was taken as 13:45 UTC, and
shows as 19:15 at +05:30.

## Intervals

An interval is a span of time, of a year-month class (YEAR, MONTH, YEAR
TO MONTH) or a day-time class (DAY, HOUR, MINUTE, SECOND, and the ranges
between them). Its qualifier names its fields: the leading one of up to
9 digits, 2 by default, `INTERVAL '3' DAY(4)`; SECOND of up to 6
fraction digits. The two classes do not mix: a month has no fixed
number of days.

A datetime plus or minus an interval of its fields is a datetime; an
interval plus or minus another of its class is an interval; an interval
may be multiplied or divided by a number. The difference of two
datetimes, in parentheses and given a qualifier, is an interval:

```sql
VALUES (DATE '2026-10-28' + INTERVAL '3' DAY, INTERVAL '1-6' YEAR TO MONTH * 2,
        INTERVAL '1:30' HOUR TO MINUTE + INTERVAL '45' MINUTE);
SELECT (TIMESTAMP '2026-10-28 12:00:00' - TIMESTAMP '2026-10-01 00:00:00') DAY TO HOUR FROM ONE;
VALUES DATE '2026-01-31' + INTERVAL '1' MONTH;
VALUES INTERVAL '1' YEAR + INTERVAL '1' DAY;
```

```
            |               |
------------+---------------+---------
 2026-10-31 |          3-00 |    2:15
(1 row)

--------
  27 12
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22008): DATE '2026-01-31' + INTERVAL '1' MONTH(2) is no valid datetime [ISO/IEC 9075:1992 §6.14 GR3c]
turborel: SQLCODE -103 (SQLSTATE 2A000): INTERVAL YEAR(2) + INTERVAL DAY(2) is no datetime or interval arithmetic [ISO/IEC 9075:1992 §6.14, §6.15] (at character 7)
```

A month added keeps the day, so January 31 plus a month would be
February 31, which is no date: SQL-92 refuses it, rather than moving to
another day.

## Assignment and CAST

**Assignment** stores a value in a column, or a parameter: INSERT,
UPDATE, a default, FETCH and SELECT INTO each assign. The value is taken
to the column's type: a string padded or its trailing spaces dropped, a
number's extra fraction digits truncated, a datetime's extra fraction
digits dropped; a value that does not fit is refused (SQLSTATE 22001 of
a string, 22003 of a number, 22026 of a bit string of the wrong
length). A value of another kind, as a number to a character column, is
refused as the statement is read. A string read into a parameter too
short for it is cut to fit instead, its indicator holding its length.

**CAST** ( *value* AS *type* ) gives the value in another type, or a
domain's, checking the domain's constraints:

| From \ To | Character | Bit | Exact | Approximate | Datetime | Interval |
|---|---|---|---|---|---|---|
| Character | yes | yes | its literal | its literal | its literal | its literal |
| Bit | yes | yes | no | no | no | no |
| Exact | yes | no | yes | yes | no | of one field |
| Approximate | yes | no | yes | yes | no | no |
| Datetime | yes | no | no | no | yes, of its fields | no |
| Interval | yes | no | of one field | no | no | of its class |

A character string is CAST to a number, datetime or interval by reading
it as a literal of that type, its leading and trailing spaces dropped.
A number CAST to an exact type is truncated to its scale.

```sql
VALUES (CAST('42' AS INTEGER), CAST(3.99 AS INTEGER), '[' || CAST(12 AS CHAR(4)) || ']',
        CAST('2026-10-28' AS DATE), CAST(TIMESTAMP '2026-10-28 13:45:00' AS TIME));
VALUES CAST('x' AS INTEGER);
VALUES CAST(12345 AS CHAR(3));
VALUES CAST(1 AS BIT(8));
```

```
              |              |        |            |
--------------+--------------+--------+------------+----------
           42 |            3 | [12  ] | 2026-10-28 | 13:45:00
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22018): 'x' is not a <signed numeric literal> [ISO/IEC 9075:1992 §6.10 GR3b i, GR4b i]
turborel: SQLCODE -301 (SQLSTATE 22001): '12345' is longer than CHARACTER(3) [ISO/IEC 9075:1992 §6.10 GR5a iv]
turborel: SQLCODE -103 (SQLSTATE 2A000): CAST of NUMERIC(1,0) to BIT(8) is not valid [ISO/IEC 9075:1992 §6.10 SR4] (at character 7)
```

The standard: ISO/IEC 9075:1992 §4.2–§4.5 and §6.1 the types, §6.10
CAST, §9.1 and §9.2 assignment, §10.1 the interval qualifier; X3.135-1986
§5.5, whose types are CHARACTER and the numbers.
