# Data Types

[Contents](README.md)

Every value is of a data type, and every column, parameter and
expression has one. A column may also be of a [domain](statements/create-domain.md),
which is a data type with a default and constraints, of a
[distinct type](#distinct-types), of a [structured type](#structured-types),
of a [reference type](#reference-types), or *(extension)* of a type a plug-in
registers, written by its name and, in parentheses, one or two unsigned
integers it takes; so may a domain.

Some such types are built in, in every database, and each is written
by its name, in any case. [UUID](#uuids) is described below.
FULLTEXT and FT_PATTERN are of [Full-Text Search](08-fulltext.md).
VECTOR, dense or sparse, is of [Vectors](14-vectors.md). ST_GEOMETRY,
also named GEOMETRY, and its 26 subtypes, of points, curves, surfaces,
solids and collections, the types beside them and the linear
referencing types are of [Spatial Data](10-spatial.md) and
[Linear Referencing](17-linear-referencing.md). A
[topology](16-topology.md) and a [spatial network](18-spatial-networks.md)
are each a schema of tables and views, and a
[property graph](20-property-graphs.md) is a schema object over tables,
so none of them is a type a column is of.

```
<data type> ::=
      <character string type> | <national character string type>
    | <binary string type> | <large object string type> | <boolean type>
    | <exact numeric type> | <approximate numeric type>
    | <decimal floating-point type>
    | <datetime type> | <interval type>
    | <row type> | <array type> | <multiset type> | <reference type>
    | <JSON type> | <XML type>
    | <path-resolved user-defined type name>
```

| Type | Holds | Written |
|---|---|---|
| CHARACTER(n) | n characters, padded with spaces | CHAR(n); CHAR is CHAR(1) |
| CHARACTER VARYING(n) | up to n characters | CHAR VARYING(n), VARCHAR(n); VARCHAR is VARCHAR(32767); VARCHAR(n OCTETS) up to n octets |
| NATIONAL CHARACTER(n) | as CHARACTER | NCHAR(n), NATIONAL CHAR(n), and VARYING |
| BINARY(n) | n octets, padded with X'00' | BINARY is BINARY(1) |
| BINARY VARYING(n) | up to n octets | VARBINARY(n); VARBINARY is VARBINARY(32767) |
| CHARACTER LARGE OBJECT(n) | up to n characters, kept in chunks | CLOB(n), NCLOB(n); n of K, M or G; CLOB is CLOB(4G) |
| BINARY LARGE OBJECT(n) | up to n octets, kept in chunks | BLOB(n); BLOB is BLOB(4G) |
| BOOLEAN | TRUE or FALSE, and UNKNOWN as its null | |
| NUMERIC(p, s), DECIMAL(p, s) | exact numbers of p digits, s after the point | DEC(p, s); (p) is (p, 0); no precision is 18 |
| INTEGER | NUMERIC(10, 0) | INT |
| SMALLINT | NUMERIC(5, 0) | |
| BIGINT | NUMERIC(18, 0) | |
| REAL | approximate numbers of 24 binary digits | FLOAT(p) for p up to 24 |
| DOUBLE PRECISION | approximate numbers of 53 binary digits | FLOAT, FLOAT(p) for p from 25 to 53 |
| DECFLOAT(p) | decimal floating point of p digits | DECFLOAT is DECFLOAT(34) |
| DATE | year, month, day | |
| TIME(p) | hour, minute, second of p fraction digits | TIME is TIME(0); WITH TIME ZONE |
| TIMESTAMP(p) | a date and a time | TIMESTAMP is TIMESTAMP(6); WITH TIME ZONE |
| INTERVAL q | a span of time, of the fields its qualifier names | INTERVAL '3' DAY |
| ROW(f t, ...) | a value of each field f, of type t | |
| t ARRAY[n] | up to n values of type t, in order | t ARRAY is t ARRAY[65535] |
| t MULTISET | up to 65,535 values of type t, in no order | see [Multisets](#multisets) |
| a distinct type | the values of its source type, compared only with each other | its name; see [Distinct types](#distinct-types) |
| a structured type | a value of each attribute, of its type or a subtype | its name; see [Structured types](#structured-types) |
| REF(t) | a reference to a row of a typed table of t | REF(t) SCOPE tab; see [Reference types](#reference-types) |
| JSON | an SQL/JSON item, stored parsed | see [JSON](#json) and [The JSON Type](12-json-type.md) |
| XML(SEQUENCE) | an XML document, fragment or sequence | XML; XML(DOCUMENT), XML(CONTENT); see [XML Values](19-xml.md) |
| UUID | a UUID of 16 octets *(extension)* | see [UUIDs](#uuids) |
| FULLTEXT | a document of up to 32,767 characters and its language | see [Full-text](#full-text) and [Full-Text Search](08-fulltext.md#the-fulltext-type) |
| FT_PATTERN | a full-text search pattern of up to 32,767 characters | see [Full-Text Search](08-fulltext.md#the-ft_pattern-type) |
| VECTOR(n, f, l) | a vector of n elements of format f, dense or sparse *(extension)* | VECTOR leaves all three open; n or f `*` leaves it open; see [Vectors](#vectors) and [Vectors](14-vectors.md#the-vector-type) |
| ST_GEOMETRY(srid) | a geometry of any type of the family, of the SRID if one is given | GEOMETRY; see [Spatial types](#spatial-types) and [Spatial Data](10-spatial.md#the-geometry-types) |
| ST_POINT(srid), ST_LINESTRING, ST_POLYGON, ... | a geometry of the subtype or a subtype of it | the 26 subtypes of ST_GEOMETRY; POINT, LINESTRING, POLYGON, MULTIPOINT and the other names of [Simple feature access](10-spatial.md#simple-feature-access) |
| ST_SPATIALREFSYS, ST_ANGLE, ST_DIRECTION, ST_VECTOR | a spatial reference system, an angle, a direction, a displacement | see [Spatial Data](10-spatial.md#systems-angles-directions-and-vectors) |
| ST_TINELEMENT, ST_AFFINEPLACEMENT, ST_NURBSPOINT, ST_KNOT | a part of a TIN, a placement, a NURBS curve | see [Spatial Data](10-spatial.md#systems-angles-directions-and-vectors) |
| ANNOTATION_TEXT, ANNOTATION_TEXT_ELEMENT, ANNOTATION_TEXT_ELEMENT_ARRAY | text placed on a map, its elements, an array of them | see [Annotation text](10-spatial.md#annotation-text) |
| ST_LRM, ST_LRCURVE, ST_POSITIONEXP, ... | a linear referencing method, a linear element, a position along it | the 13 types of [Linear Referencing](17-linear-referencing.md) |

The longest string is 32,767 characters, the longest binary string 32,767
octets, the longest large object 4G characters or octets, an exact number holds at most 18 digits and a DECFLOAT at most
34. A type past a limit
is valid SQL Turborel does not process: SQLCODE -900, SQLSTATE 0A000.
The [Limits](../appendix/d-limits.md) appendix lists them all.

## Character strings

A character string is a sequence of Unicode characters. CHARACTER(n)
holds exactly n: a shorter value is padded with spaces as it is stored.
CHARACTER VARYING(n) holds the value as written, up to n characters.
Written without a length, it holds the longest string, 32,767
characters, as `CAST(x AS VARCHAR)` does. A value is stored at its own
length, so a longer maximum costs nothing. Strings are compared as if the shorter were
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

A length may count octets of the string's UTF-8 instead, written
`VARCHAR(10 OCTETS)` or `CHAR(10 OCTETS)`, and `10 CHARACTERS` is the
default spelled out. A fixed string is then padded to so many octets,
and a value or CAST is cut at whole characters, so `é`, two octets, is
never split:

```sql
CREATE TABLE BYTES (V VARCHAR(4 OCTETS), F CHAR(4 OCTETS));
INSERT INTO BYTES VALUES ('éé', 'é');
SELECT '[' || F || ']' AS FIXED, OCTET_LENGTH(V) AS VO, CHAR_LENGTH(F) AS FC FROM BYTES;
INSERT INTO BYTES VALUES ('ééé', 'a');
```

```
OK
OK
 FIXED  |                   VO |                   FC
--------+----------------------+----------------------
 [é  ]  |                    4 |                    3
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22001): a value of 6 octets, not spaces beyond 4, does not fit CHARACTER VARYING(4 OCTETS) [ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3e ii]
```

Strings brought together, by CASE, UNION or `||`, keep a length in
octets if all of them have one, and otherwise count characters.

NATIONAL CHARACTER is CHARACTER: there is one character set, SQL_TEXT,
which holds every Unicode character, so the national one is the same.
The Information Schema shows such a column as CHARACTER.

### Collations

A column of a character string compares by its collation, SQL_TEXT, the order
of Unicode scalar values, unless it says otherwise:

```sql
CREATE TABLE PEOPLE (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE UNIQUE);
SELECT CITY FROM PLACES ORDER BY CITY COLLATE UNICODE;
```

| Collation | Order |
|---|---|
| SQL_TEXT, UCS_BASIC | Unicode scalar value, so `Z` before `a` |
| UNICODE | Language-neutral, by base letter, then accent, then case |
| UNICODE_SV, UNICODE_ZH, UNICODE_DE_PHONEBOOK, ... | UNICODE as a language orders it, such as Swedish `å` after `z` or Chinese by pinyin |
| CASE_INSENSITIVE | Scalar value of each character's case folding |

In a comparison a literal takes the column's collation. Two columns of
different collations need COLLATE. A collation's order never changes: a new
Unicode version would be a new collation. See
[CREATE COLLATION](statements/create-collation.md) and
[SET COLLATION](statements/set-collation.md).

## Binary strings

A binary string is a sequence of octets, written `X'A0FF'`, two hexits
to an octet. Spaces may stand among the hexits, and parts on separate
lines are one literal, as a character string's are. A literal is
BINARY of its octets. A value of BINARY(n) has exactly n octets: one
shorter is padded with X'00' octets, and one longer is cut if the octets
past n are all X'00', else refused. BINARY VARYING(n), or VARBINARY(n),
holds up to n, cut the same way. Written without a length, it holds the
longest binary string, 32,767 octets.

Binary strings compare octet by octet, and a string is less than a
longer one it begins, so `X'01'` is less than `X'0100'`. `||` joins
two, SUBSTRING and POSITION count octets, and OCTET_LENGTH gives a
binary string's length. BIT_LENGTH, which counts eight bits to an
octet, is kept from SQL-92.

```sql
CREATE TABLE BT (B BINARY(4), V VARBINARY(16));
INSERT INTO BT VALUES (X'A0', X'FF 00');
SELECT B, V, OCTET_LENGTH(V), B || V, POSITION(X'00' IN V) FROM BT;
INSERT INTO BT (B) VALUES (X'0102030405');
```

```
OK
OK
 B        | V                                |                      |                                          |
----------+----------------------------------+----------------------+------------------------------------------+----------------------
 A0000000 | FF00                             |                    2 | A0000000FF00                             |                    2
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22001): a binary string of 5 octets, not X'00' beyond 4, does not fit BINARY(4) [ISO/IEC 9075:1992 §13.8 GR4b iii; ISO/IEC 9075-2:2023 §9.2 GR3b ix 2]
```

A binary string is neither a number nor characters. CAST takes one only
to another binary type, and nothing else to one. SQL-92's BIT and BIT
VARYING, and its `B'...'` literals, are gone, as SQL:2023 has none.

## Large objects

CHARACTER LARGE OBJECT, or CLOB, holds text, and BINARY LARGE OBJECT,
or BLOB, octets, each up to 4G of them. A length may end in K, M or G,
1024 and its powers, and a CLOB's may say CHARACTERS, the default, or
OCTETS. NATIONAL CHARACTER LARGE OBJECT, or NCLOB, is a CLOB. Without a
length either holds the longest, 4G.

```
<large object string type> ::=
      { CHARACTER | CHAR } LARGE OBJECT [ ( <large object length> ) ] | CLOB [ ( ... ) ]
    | { NATIONAL CHARACTER | NCHAR } LARGE OBJECT [ ( ... ) ] | NCLOB [ ( ... ) ]
    | BINARY LARGE OBJECT [ ( ... ) ] | BLOB [ ( ... ) ]
<large object length> ::= <unsigned integer> [ K | M | G ] [ CHARACTERS | OCTETS ]
```

A large object is kept apart from its row, in chunks of 256 KiB, and is
logged, read and sent to a client a chunk at a time, so none is ever
held whole in memory. A value a function makes, as UPPER or LPAD make
one, is written to a temporary file the same way. A large object inside
an array, a multiset, a row or a structured value is kept whole in that
value's row instead, and one longer than 256 KiB is refused there
(SQLCODE -900). The column store keeps no large object.

`||`, SUBSTRING, POSITION, OVERLAY, TRIM, UPPER, LOWER, the pads, LIKE
and the regular expressions take large objects, and a CLOB and a
character string, or a BLOB and a binary string, mix as two strings do.
`||` and SUBSTRING share the chunks they read rather than copy them, and
CHAR_LENGTH and OCTET_LENGTH read nothing. Large objects compare for
equality alone. ORDER BY, GROUP BY, DISTINCT, MAX, MIN, `<` and a key of
a constraint or an index refuse one (SQLCODE -103).

```sql
CREATE TABLE NOTES (ID INTEGER, BODY CLOB(16M), SCAN BLOB);
INSERT INTO NOTES VALUES (1, 'Ada wrote the first program.', X'FFD8FF');
SELECT ID, CHAR_LENGTH(BODY), OCTET_LENGTH(SCAN) FROM NOTES WHERE BODY LIKE '%program%';
SELECT CAST(UPPER(SUBSTRING(BODY FROM 1 FOR 3)) AS VARCHAR(10)) FROM NOTES WHERE BODY = 'Ada wrote the first program.';
SELECT ID FROM NOTES ORDER BY BODY;
```

```
OK
OK
           ID |                      |
--------------+----------------------+----------------------
            1 |                   28 |                    3
(1 row)

------------
 ADA
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): ORDER BY orders values of CHARACTER LARGE OBJECT(16777216), and large objects are compared for equality alone [ISO/IEC 9075-2:2023 §9.14 SR3]
```

A value stored in a site shorter than it is refused (SQLSTATE 22001),
unless what is past the site is spaces, which are cut. One cast shorter
is cut with the warning 01004. A client reads a long value by its
locator, a chunk at a time, and may write one the same way; FREE
LOCATOR and HOLD LOCATOR end a locator early or keep it past a commit.
The [client library](../client/02-library.md#large-objects) and the
[embedded library](../client/03-embedding.md#large-objects) say how.

## Booleans

BOOLEAN holds the truth values TRUE and FALSE. UNKNOWN is its null
value, so `UNKNOWN` and `NULL` written into a column are the same. TRUE
is greater than FALSE, and a truth value compares with truth values
alone.

A condition is a value wherever a value is written: in a select list, a
VALUES row, an UPDATE's SET, a CASE result or a CAST operand. A BOOLEAN
value is a condition wherever one is written, so `WHERE DONE` reads a
column. A value of any other type there is refused.

EVERY gives TRUE if each value of its group is TRUE, ANY and SOME if one
is. Each leaves nulls out, and is null of a group with no other values.

CAST takes a character string to BOOLEAN if it is TRUE, FALSE or
UNKNOWN, in any case and with spaces either side, and a truth value to a
string as `'TRUE'` or `'FALSE'`. BOOLEAN and EVERY are not reserved
words, so a column of an older database named either keeps its name.

```sql
CREATE TABLE TASK (ID INTEGER, DONE BOOLEAN DEFAULT FALSE);
INSERT INTO TASK VALUES (1, TRUE), (2, FALSE), (3, UNKNOWN);
SELECT ID, DONE, NOT DONE, ID > 1 AND DONE IS NOT TRUE FROM TASK;
SELECT ID FROM TASK WHERE DONE;
SELECT EVERY(DONE), ANY(DONE), COUNT(DONE) FROM TASK;
VALUES CAST(' true ' AS BOOLEAN);
VALUES CAST('yes' AS BOOLEAN);
SELECT ID FROM TASK WHERE ID;
```

```
OK
OK
           ID | DONE  |       |
--------------+-------+-------+-------
            1 | TRUE  | FALSE | FALSE
            2 | FALSE | TRUE  | TRUE
            3 |       |       | TRUE
(3 rows)
           ID
--------------
            1
(1 row)
       |       |
-------+-------+----------------------
 FALSE | TRUE  |                    2
(1 row, with a warning: SQLSTATE 01003)

-------
 TRUE
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22018): 'yes' is not a <boolean literal> [ISO/IEC 9075-2:2023 §6.13 GR21a ii]
turborel: SQLCODE -103 (SQLSTATE 2A000): a <boolean predicand> is of type BOOLEAN (found INTEGER) [ISO/IEC 9075-2:2023 §6.46 SR3] (at character 26)
```

The warning is SQLSTATE 01003, a null left out of a set function.

## Numbers

**Exact numbers** are decimal: NUMERIC(p, s) and DECIMAL(p, s) hold p
digits, s of them after the decimal point, and are the same type.
INTEGER is NUMERIC(10, 0), SMALLINT NUMERIC(5, 0) and BIGINT
NUMERIC(18, 0). A value with more
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
of a sum or a literal, as NUMERIC or FLOAT. Values of one type written
alike keep its name when brought together, so a UNION of two INTEGER
columns is INTEGER, and of INTEGER and SMALLINT NUMERIC. The name is no part of the
type: INTEGER and NUMERIC(10, 0) are one type, so a foreign key of one
references the other.

BIGINT holds every number of 18 digits, from -999999999999999999 to
999999999999999999. That is the most digits an exact value holds here,
so a 64-bit integer past them, as 9223372036854775807 is, does not fit
and is refused. BIGINT is no key word, so a column may be named BIGINT.

DECFLOAT(p) is decimal floating point. A value has at most p digits, p
from 1 to 34, with a decimal exponent, so it keeps decimal fractions
exactly as an exact number does yet is not held to one scale. DECFLOAT
alone is DECFLOAT(34). Its values range from 1E-6143 to
9.999999999999999999999999999999999E6144, of either sign, and 0. A
value with more digits than its type holds is rounded half to even, so
1.25 at two digits is 1.2 and 1.35 is 1.4. 1.5 and 1.50 are one value,
shown as 1.5.

A sum, difference, product or quotient with a DECFLOAT operand is
DECFLOAT(34), the value of 34 digits nearest the exact result, so
0.1 + 0.2 is exactly 0.3. An exact or approximate operand is taken as
DECFLOAT first, an approximate one from its binary value, so 0.1E0 is
0.1000000000000000055511151231257827. SUM and AVG of DECFLOAT values are
DECFLOAT(34) too. A DECFLOAT compares with any number by value. A
result past the exponent's range is refused (SQLSTATE 22003), and so is
a divisor of 0 (22012).

A DECFLOAT value is shown plainly unless it has more than 34 digits
before the point or more than 6 zeros after it, and then in E notation.
DECFLOAT has no literal of its own. A numeric literal, or a CAST of a
string, gives its value. DECFLOAT is not reserved, so a column may be
named DECFLOAT.

```sql
CREATE TABLE PRICE (ITEM INTEGER, AMOUNT DECFLOAT, RATE DECFLOAT(4));
INSERT INTO PRICE VALUES (1, 0.1, 1.23456), (2, 0.2, 2.5);
SELECT SUM(AMOUNT), AVG(AMOUNT), MAX(RATE) FROM PRICE;
SELECT AMOUNT / 3, AMOUNT * 1E30, AMOUNT / 1E10, CAST(AMOUNT AS DOUBLE PRECISION) FROM PRICE WHERE ITEM = 1;
SELECT AMOUNT / 0 FROM PRICE;
```

```
OK
OK
                                             |                                             |
---------------------------------------------+---------------------------------------------+---------------
                                         0.3 |                                        0.15 |           2.5
(1 row)
                                             |                                             |                                             |
---------------------------------------------+---------------------------------------------+---------------------------------------------+--------------------------
       0.03333333333333333333333333333333333 |            100000000000000001988462483865.6 |                                       1E-11 |                     1E-1
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22012): a divisor shall not be 0 [ISO/IEC 9075-2:2023 §6.30 GR4]
```

1.23456 was rounded to RATE's four digits, 1.235. 1E30 is approximate,
so its binary value, 1000000000000000019884624838656, is the factor.

## Datetimes

DATE holds a date of the years 1 to 9999; TIME(p) a time of day, its
seconds of p fraction digits, at most 6; TIMESTAMP(p) a date and a time.
WITH TIME ZONE adds a time zone displacement, from -12:59 to +14:00.
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
turborel: SQLCODE -103 (SQLSTATE 2A000): DATE '2026-02-30' is no valid <datetime literal> [ISO/IEC 9075:1992 §5.3 SR21, SR22; ISO/IEC 9075-2:2023 §5.3 SR43] (at character 27)
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

## Arrays and rows

An array holds elements of one type, in order, from none to its
maximum cardinality: `INTEGER ARRAY[3]` holds up to three integers. Any
type of the standard may be an element's, an array or a row among them,
so `INTEGER ARRAY[2] ARRAY[4]` is an array of arrays. An element may be
null, and so may the array itself, which differs from an array of no
elements. A row type holds a value of each of its fields, each named
and of its own type: `ROW(X INTEGER, Y INTEGER)`.

```sql
CREATE TABLE ROUTES (ID INTEGER PRIMARY KEY, STOPS VARCHAR(20) ARRAY[10],
                     START ROW(X DECIMAL(9,6), Y DECIMAL(9,6)));
```

Arrays are compared for equality alone, element by element. Two arrays
are equal if they have as many elements and each pair is equal. They are
never ordered or grouped, so no array is in ORDER BY, GROUP BY, DISTINCT,
UNION, a key or an index. Rows are compared field by field, the first
unequal field deciding which is the less, and may be ordered and grouped.
Types nest at most 16 deep.

[Value Expressions](04-expressions.md#arrays-and-rows) makes and reads
them, and UNNEST in [Query Expressions](06-queries.md#unnest) gives an
array's elements as rows.

## Multisets

A multiset holds elements of one type in no order, each as many times as
it was put in: `VARCHAR(10) MULTISET` holds strings, the same one more
than once if need be. Any type of the standard may be an element's, an
array, a row or another multiset among them. An element may be null, and
so may the multiset itself, which differs from a multiset of no
elements. A multiset holds at most 65,535 elements.

```sql
CREATE TABLE BAGS (ID INTEGER PRIMARY KEY, TAGS VARCHAR(10) MULTISET);
INSERT INTO BAGS VALUES (1, MULTISET['red', 'blue', 'red']), (2, MULTISET[]), (3, NULL);
SELECT ID, TAGS, CARDINALITY(TAGS) AS N FROM BAGS ORDER BY ID;
SELECT ID FROM BAGS WHERE TAGS = MULTISET['red', 'red', 'blue'];
SELECT TAGS FROM BAGS ORDER BY TAGS;
```

```
OK
OK
           ID | TAGS                                                         |            N
--------------+--------------------------------------------------------------+--------------
            1 | MULTISET['red ', 'blue', 'red ']                             |            3
            2 | MULTISET[]                                                   |            0
            3 | NULL                                                         |         NULL
(3 rows)
           ID
--------------
            1
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): ORDER BY orders values of CHARACTER VARYING(10) MULTISET, and multisets are compared for equality alone [ISO/IEC 9075-2:2023 §9.14 SR3]
```

The literals `'red'` and `'blue'` are brought together as CHARACTER(4),
so `'red'` is padded, as an array's elements would be. Multisets are
compared for equality alone. Two are equal if their elements pair off,
each pair equal, whatever their order, so the second query finds row 1.
They are never ordered or grouped, so no multiset is in ORDER BY, GROUP
BY, DISTINCT, UNION, a key or an index. A distinct type may have a
multiset type as its source.

[Value Expressions](04-expressions.md#multisets) makes and combines
them, [Predicates](05-predicates.md#member-submultiset-and-is-a-set)
tests them, and UNNEST in [Query Expressions](06-queries.md#unnest)
gives their elements as rows.

## Distinct types

A distinct type is a type of its own made from another, its source type,
by [CREATE TYPE](statements/create-type.md). `CREATE TYPE MONEY AS
DECIMAL(10,2)` makes MONEY, whose values are those of DECIMAL(10,2) but
compare only with other values of MONEY. A price and a weight of one
source type cannot then be compared or added by mistake.

The source type is a predefined type, XML among them, an array type or a multiset type. A value of the
source, or of a type that is assigned to it, is assigned to a column of
the distinct type, and a value of the distinct type is assigned only to a
site of exactly its source type. CAST takes the same paths. No operator
takes a value of a distinct type but comparison, so `PRICE + 1` is
refused and `"NUMERIC"(PRICE) + 1` is not. Values of a type whose source
is an array type are compared with none, so they are never ordered or
grouped. One whose source is a multiset type is taken by the multiset
operators and predicates as a multiset is.

The type's definition makes two functions, one from its source, named
for the type, and one to it, named for the source type. Each is invoked
by name, `MONEY(5)` and `"NUMERIC"(PRICE)`, and needs EXECUTE, which the
type's owner holds and [GRANT](statements/grant.md) gives. A column of the
type needs USAGE on it. A client reads and writes its values as values of
the source type.

## Structured types

A structured type has attributes, each of a data type, and methods, and
may be a subtype of another, by [CREATE TYPE](statements/create-type.md).
`CREATE TYPE ADDRESS AS (STREET VARCHAR(30), CITY VARCHAR(20)) NOT
FINAL` makes ADDRESS. `NEW ADDRESS()` makes a value of its attributes'
defaults, `a.CITY` observes an attribute, and `a.CITY('OSLO')` gives a
copy of the value with the attribute changed. A method is invoked as
`a.m(args)`, a static one as `ADDRESS::m(args)`, and NEW with arguments
invokes a constructor method.

A value of a subtype is a value of each of its supertypes, and is stored
in a column of any of them. It keeps its most specific type, which
`v.SPECIFICTYPE` names and `v IS OF (t)` tests, and its methods run by
that type, so an OVERRIDING method of a subtype runs for its values.
`TREAT(v AS t)` takes a value as one of its subtype `t`, and is refused
(SQLSTATE 0D000) of a value of no subtype of it.

Values of a structured type compare by its ordering, which
[CREATE ORDERING](statements/create-ordering.md) gives, and by nothing
until then. A value is stored whole in its row. A typed table
([CREATE TABLE](statements/create-table.md) `OF` a type) holds one
value of the type in each row, its attributes as its columns, and the
typed tables of a type and its subtypes form a family of supertables and
subtables.

## Reference types

`REF(t)` is the type of a reference to a row of a typed table of the
structured type `t`, as a typed table's self-referencing column holds
one. `REF(t) SCOPE tab` names the table it refers into. `r -> a` reads
the attribute `a` of the row `r` identifies, `r -> m()` invokes a
method of it, and `DEREF(r)` gives the row as a value of `t`, each a
null value if no row is identified. A reference without a scope is not
dereferenced (SQLSTATE 2A000). References compare for equality alone,
and may be elements of arrays and multisets.

## XML

XML, XML(DOCUMENT), XML(CONTENT) and XML(SEQUENCE) hold XML documents
and fragments of them. [XML Values](19-xml.md) describes them, and XMLPARSE,
XMLDOCUMENT, XMLCAST, the publishing operators and XMLSERIALIZE, which
make and write them.

## JSON

JSON holds an SQL/JSON item: an object, an array, a string, a number,
true, false or the JSON null, which is a value and not the SQL null
value. A value is kept parsed, as a tree its accessors read in place,
never as JSON text, and an object keeps its members in the order they
were written. JSON ( ) parses a character string's JSON text into a
value, JSON_SCALAR makes one of a string, number, truth value or
datetime, and JSON_SERIALIZE writes one's text. Any two values of JSON
compare, by the items they are, so JSON columns take ORDER BY, GROUP
BY, DISTINCT and keys. [The JSON Type](12-json-type.md) describes it,
and [The Native Protocol](../client/05-protocol.md#json-values) the
form a client reads.

## UUIDs

*(extension)* UUID holds a universally unique identifier of RFC 9562,
16 octets. Its literal is `UUID '919108f7-52d1-4320-9bac-f847db4148a8'`.
Text cast to UUID, or stored in a UUID column, is read in the hyphenated
form, in upper or lower case, in braces, or as 32 hexadecimal digits
with no hyphens. A binary string of 16 octets casts to UUID. A UUID
casts to a character string as its hyphenated text in lower case, and to
a binary string as its octets. Text that is no UUID's is refused with
SQLSTATE 22018, as is a binary string of another length.

UUIDs compare, sort and group by their octets, so they take a primary
key, a unique constraint and an index of either method, B-link or hash.
The Information Schema gives a UUID column's DATA_TYPE as UUID, its
lengths and precisions null. [UUID_V4 and UUID_V7](04-expressions.md#uuid_v4-and-uuid_v7)
make new UUIDs.

```sql
CREATE TABLE TICKETS (ID UUID PRIMARY KEY, SEAT VARCHAR(4));
INSERT INTO TICKETS VALUES ('{919108F7-52D1-4320-9BAC-F847DB4148A8}', '12A'),
    ('0f0e0d0c0b0a49088706050403020100', '3C');
SELECT ID, SEAT FROM TICKETS ORDER BY ID;
SELECT SEAT, CAST(ID AS VARBINARY(16)) FROM TICKETS WHERE ID = UUID '919108f7-52d1-4320-9bac-f847db4148a8';
INSERT INTO TICKETS VALUES ('not-a-uuid', '1A');
```

```
OK
OK
 ID                                   | SEAT
--------------------------------------+------
 0f0e0d0c-0b0a-4908-8706-050403020100 | 3C
 919108f7-52d1-4320-9bac-f847db4148a8 | 12A
(2 rows)
 SEAT |
------+----------------------------------
 12A  | 919108F752D143209BACF847DB4148A8
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22018): 'not-a-uuid' is not a UUID's text
```

## Full-text

*(extension)* FULLTEXT holds a document, of at most 32,767 characters,
and the name of its language, ENGLISH unless another is given. It is
made by `FULLTEXT(text [, language])`, `NEW FullText(text [, language])`
or a CAST of text, and searched by its methods, Contains and Rank, and
by a fulltext index. FT_PATTERN holds a search pattern of at most 32,767
characters, which a character string is assigned to and cast to. Both
are the types of SQL/MM Full-Text, built in; [Full-Text
Search](08-fulltext.md) describes them.

## Vectors

*(extension)* VECTOR holds a vector of numbers, such as an embedding a
model made. `VECTOR(n, f, l)` fixes its dimension count n, the format f
of its elements, FLOAT32, FLOAT16, FLOAT64, INT8 or BINARY, and its
layout l, DENSE or SPARSE. The format is FLOAT32 and the layout DENSE
unless written, `*` leaves the dimension count or the format open, and
VECTOR alone leaves all three open. A sparse vector keeps only its
elements that are not zero, with their positions, so there is no
separate sparse type. A vector is written as text, `'[0.12, -0.5,
3.0E-2]'`. Vectors have no order and no equality, so no vector is in
ORDER BY, GROUP BY, DISTINCT or a key. [Vectors](14-vectors.md)
describes them, their distances and their indexes.

## Spatial types

ST_GEOMETRY, also named GEOMETRY, holds a geometry: a point, a curve, a
surface, a solid or a collection of them, with its coordinates and its
SRID. Its 26 subtypes, ST_POINT, ST_LINESTRING, ST_POLYGON,
ST_MULTIPOINT, ST_MULTILINESTRING, ST_MULTIPOLYGON, ST_GEOMCOLLECTION,
the curves ST_CIRCULARSTRING, ST_COMPOUNDCURVE, ST_NURBSCURVE and the
others, the surfaces, ST_TIN and the solids, each hold geometries of
their own type and the types below it. Each takes an SRID as its
parameter, `ST_POINT(4326)`, and then holds only values of that SRID.
The names of Simple feature access, POINT, LINESTRING, POLYGON,
POLYHEDSURFACE, GEOMCOLLECTION and the multi types, name the same types.
Geometries compare for equality alone, so no geometry is in ORDER BY,
GROUP BY, DISTINCT or a key. [Spatial Data](10-spatial.md#the-geometry-types)
gives the whole hierarchy.

Beside them are ST_SPATIALREFSYS, ST_ANGLE, ST_DIRECTION and ST_VECTOR,
the parts ST_TINELEMENT, ST_AFFINEPLACEMENT, ST_NURBSPOINT and ST_KNOT,
and the annotation types ANNOTATION_TEXT, ANNOTATION_TEXT_ELEMENT and
ANNOTATION_TEXT_ELEMENT_ARRAY, each described in [Spatial
Data](10-spatial.md). The 13 types of [Linear
Referencing](17-linear-referencing.md) are ST_LRM, ST_LINEARELEMENT,
ST_LRFEATURE, ST_LRCURVE, ST_LRDIRECTEDEDGE, ST_POSITIONEXP,
ST_LRMEASURE, ST_STARTVALUE, ST_DISTANCEEXP, ST_REFERENT,
ST_LATOFFSETEXP, ST_VEROFFSETEXP and ST_VECTOROFFSETEXP. Topologies and
spatial networks add no types of their own.

## Assignment and CAST

**Assignment** stores a value in a column, or a parameter: INSERT,
UPDATE, a default, FETCH and SELECT INTO each assign. The value is taken
to the column's type: a string padded or its trailing spaces dropped, a
number's extra fraction digits truncated, a datetime's extra fraction
digits dropped; a value that does not fit is refused (SQLSTATE 22001 of
a string, 22003 of a number). A value of another kind, as a number to a character column, is
refused as the statement is read. A string read into a parameter too
short for it is cut to fit instead, its indicator holding its length,
with the warning 01004, a binary string too; a binary string shorter
than a BINARY parameter is padded with X'00' octets, with no warning.

**CAST** ( *value* AS *type* ) gives the value in another type, or a
domain's, checking the domain's constraints:

| From \ To | Character | Binary | Exact | Approximate | DECFLOAT | Datetime | Interval |
|---|---|---|---|---|---|---|---|
| Character | yes | no | its literal | its literal | its literal | its literal | its literal |
| Binary | no | yes | no | no | no | no | no |
| Exact | yes | no | yes | yes | yes | no | of one field |
| Approximate | yes | no | yes | yes | yes | no | no |
| DECFLOAT | yes | no | yes | yes | yes | no | no |
| Datetime | yes | no | no | no | no | yes, of its fields | no |
| Interval | yes | no | of one field | no | no | no | of its class |

A distinct type is assigned and CAST from its source, and to it, as
[Distinct types](#distinct-types) says. An array is assigned and CAST
element by element to its target's element type, and a row field by field to its target's fields, in order, whatever
their names. An array of more elements than its target's maximum
cardinality is refused, SQLSTATE 2202F, but on assignment its nulls past
the maximum are dropped. Nothing else is assigned or CAST to or from an
array or a row.

A character string is CAST to a number, datetime or interval by reading
it as a literal of that type, its leading and trailing spaces dropped.
A number CAST to an exact type is truncated to its scale, and to
DECFLOAT rounded half to even to its precision.

```sql
VALUES (CAST('42' AS INTEGER), CAST(3.99 AS INTEGER), '[' || CAST(12 AS CHAR(4)) || ']',
        CAST('2026-10-28' AS DATE), CAST(TIMESTAMP '2026-10-28 13:45:00' AS TIME));
VALUES CAST('x' AS INTEGER);
VALUES CAST(12345 AS CHAR(3));
VALUES CAST(1 AS BINARY(8));
```

```
              |              |        |            |
--------------+--------------+--------+------------+----------
           42 |            3 | [12  ] | 2026-10-28 | 13:45:00
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22018): 'x' is not a <signed numeric literal> [ISO/IEC 9075:1992 §6.10 GR3b i, GR4b i]
turborel: SQLCODE -301 (SQLSTATE 22001): '12345' is longer than CHARACTER(3) [ISO/IEC 9075:1992 §6.10 GR5a iv]
turborel: SQLCODE -103 (SQLSTATE 2A000): CAST of NUMERIC(1,0) to BINARY(8) is not valid [ISO/IEC 9075-2:2023 §6.13 SR7] (at character 7)
```

A string CAST to a shorter character type is cut to its length, the
statement completing with the warning 01004 if a character cut is not
a space. A binary string CAST to a shorter binary type is cut, with the
warning 01004, and to a longer BINARY padded with X'00' octets, with no
warning.

```sql
VALUES CAST('abcdef' AS CHAR(3));
VALUES CAST('abc   ' AS VARCHAR(3));
VALUES CAST(X'010203' AS BINARY(2));
VALUES CAST(X'01' AS BINARY(3));
```

```

-----
 abc
(1 row, with a warning: SQLSTATE 01004)

-----
 abc
(1 row)

------
 0102
(1 row, with a warning: SQLSTATE 01004)

--------
 010000
(1 row)
```

The standard: ISO/IEC 9075:1992 §4.2–§4.5 and §6.1 the types, §6.10
CAST, §9.1 and §9.2 assignment, §10.1 the interval qualifier; ISO/IEC
9075-2:2023 §4.4, §4.6 and §6.1 the binary string and Boolean types,
§6.13 their CAST, §9.1 and §9.2 their assignment, §4.5 and §6.30 the
decimal floating-point type and its arithmetic, §4.7.2 the range of time
zone displacements (IL061, IL062); X3.135-1986 §5.5,
whose types are CHARACTER and the numbers.
