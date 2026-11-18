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

LIKE of binary strings matches octets. `_` is the octet X'5F' and `%`
X'25', and an escape octet, one octet long or SQLSTATE 2200D, takes
either or itself as written. A pattern of the wrong family is refused.
A [large object](02-data-types.md#large-objects) is matched as it is
read, a chunk at a time, however long. Its pattern and escape, and under
UNICODE or a tailoring its match value too, are held whole, and are
refused past 262,144 octets (SQLSTATE 0A000).

```sql
VALUES (CASE WHEN X'0A250B' LIKE X'0A215F25' ESCAPE X'21' THEN 'yes' ELSE 'no' END,
        CASE WHEN X'0A250B' LIKE X'0A21250B' ESCAPE X'21' THEN 'yes' ELSE 'no' END);
```

```
     |
-----+-----
 no  | yes
(1 row)
```

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

## JSON_EXISTS

`JSON_EXISTS(text, 'path' [PASSING ...] [behavior ON ERROR])` is true if
the SQL/JSON path finds an item in the JSON text, false if it finds
none, and unknown if the text is null. The path and PASSING are as
[JSON_VALUE](04-expressions.md#sqljson) reads them. In place of a data
exception, as text that is no JSON or a strict path's member that is
not there, it is FALSE, the default, TRUE, UNKNOWN or the ERROR raised,
as ON ERROR says.

```sql
SELECT ID FROM PURCHASES WHERE JSON_EXISTS(DOC, 'lax $.items[*] ? (@.qty > 1)') ORDER BY ID;
SELECT ID, JSON_EXISTS(DOC, 'lax $.note'), JSON_EXISTS(DOC, 'strict $.note.x'),
       JSON_EXISTS(DOC, 'strict $.note.x' UNKNOWN ON ERROR) FROM PURCHASES ORDER BY ID;
```

```
           ID
--------------
            1
(1 row)
           ID |       |       |
--------------+-------+-------+-------
            1 | FALSE | FALSE | NULL
            2 | TRUE  | FALSE | NULL
(2 rows)
```

A member whose value is JSON's null is there, so `lax $.note` is true of
the second row.

## IS JSON

`s [FORMAT JSON] IS [NOT] JSON [VALUE | ARRAY | OBJECT | SCALAR] [WITH
UNIQUE [KEYS] | WITHOUT UNIQUE [KEYS]]` is true if the character string
s is JSON text of the kind named, any kind, VALUE, if none is: an array,
an object, or a scalar, which is neither. It is false of text of
another kind or no JSON text, and WITH UNIQUE KEYS of an object with two
members of one name. It is unknown if s is null. IS NOT JSON is its
negation.

```sql
SELECT ID, DOC IS JSON OBJECT, DOC IS JSON ARRAY, 'nope' IS NOT JSON, '{"a":1,"a":2}' IS JSON WITH UNIQUE KEYS FROM PURCHASES ORDER BY ID;
```

```
           ID |       |       |       |
--------------+-------+-------+-------+-------
            1 | TRUE  | FALSE | TRUE  | FALSE
            2 | TRUE  | FALSE | TRUE  | FALSE
(2 rows)
```

A CHECK constraint may hold it, as `CHECK (DOC IS JSON OBJECT)`.

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

## Period predicates

A period runs from its start, included, to its end, excluded. It is
named by a period of a table (see [CREATE TABLE](statements/create-table.md)),
or written `PERIOD (start, end)` of two dates or two timestamps of one
type. `p OVERLAPS q` is true if they share an instant, `p EQUALS q` if
their bounds are the same, `p CONTAINS q` if q lies within p, and
`p CONTAINS x` if the instant x does. `p PRECEDES q` is true if p ends
before or as q starts, `p SUCCEEDS q` if p starts at or after q's end,
and IMMEDIATELY PRECEDES and IMMEDIATELY SUCCEEDS if they meet exactly.
A written period with a null bound makes the predicate unknown, and one
whose start is not before its end is an error (SQLSTATE 22020).

```sql
VALUES CASE WHEN PERIOD (DATE '2026-01-01', DATE '2026-01-05')
                 IMMEDIATELY PRECEDES PERIOD (DATE '2026-01-05', DATE '2026-01-09')
            THEN 'meets' ELSE 'apart' END;
```

```

-------
 meets
(1 row)
```

```sql
VALUES CASE WHEN PERIOD (DATE '2026-01-01', DATE '2026-01-05') CONTAINS DATE '2026-01-05'
            THEN 'in' ELSE 'out' END;
```

```

-----
 out
(1 row)
```

## MEMBER, SUBMULTISET and IS A SET

`x MEMBER OF m` is TRUE if an element of the multiset m equals x, FALSE
if m is empty, and else unknown if x, m or an element is null.
`m SUBMULTISET OF n` is TRUE if each element of m pairs with an equal
element of n, a different one each, so `MULTISET[1, 1]` is no
submultiset of `MULTISET[1, 2]`. An empty m is a submultiset of
anything. `m IS A SET` is TRUE if no two elements are alike, nulls
counting as alike. Each takes NOT, and OF may be left out.

```sql
CREATE TABLE BAGS (ID INTEGER PRIMARY KEY, TAGS VARCHAR(10) MULTISET);
INSERT INTO BAGS VALUES (1, MULTISET['red', 'blue', 'red']), (2, MULTISET['blue']), (3, MULTISET[]);
SELECT ID FROM BAGS WHERE 'red' MEMBER OF TAGS ORDER BY ID;
SELECT ID FROM BAGS WHERE TAGS SUBMULTISET OF MULTISET['red', 'blue'] ORDER BY ID;
SELECT ID FROM BAGS WHERE TAGS IS A SET ORDER BY ID;
```

```
OK
OK
           ID
--------------
            1
(1 row)
           ID
--------------
            2
            3
(2 rows)
           ID
--------------
            2
            3
(2 rows)
```

## Type predicate

`v IS OF (t, ...)` is true if the most specific type of the value of a
structured type is one of the types or a subtype of one, and `ONLY t`
names the type alone. IS NOT OF is its negation. Of the orders of
[Structured values and references](04-expressions.md#structured-values-and-references):

```sql
SELECT N, TREAT(DEREF(WHAT) AS GIFT).WRAP FROM ORDERS WHERE DEREF(WHAT) IS OF (GIFT);
SELECT N FROM ORDERS WHERE DEREF(WHAT) IS OF (ONLY ITEM);
```

```
            N |
--------------+------------
            2 | RED
(1 row)
            N
--------------
            1
(1 row)
```

## DOCUMENT and CONTENT

`x IS DOCUMENT` is true if the [XML value](16-xml.md) x is a document: one
document node whose children are one element, with comments and
processing instructions beside it. `x IS CONTENT` is true if x is one
document node, of any children. Each is unknown if x is null, and takes
NOT. The operand is one XML value, else SQLCODE -103.

```sql
CREATE TABLE PAGES (ID INTEGER, BODY XML);
INSERT INTO PAGES VALUES (1, XMLPARSE(CONTENT '<p>one</p>' PRESERVE WHITESPACE)), (2, XMLPARSE(CONTENT 'two <b/>' PRESERVE WHITESPACE)), (3, NULL);
SELECT ID FROM PAGES WHERE BODY IS DOCUMENT;
SELECT ID FROM PAGES WHERE BODY IS NOT DOCUMENT;
SELECT ID FROM PAGES WHERE BODY IS CONTENT ORDER BY ID;
SELECT ID FROM PAGES WHERE ID IS DOCUMENT;
```

```
OK
OK
           ID
--------------
            1
(1 row)
           ID
--------------
            2
(1 row)
           ID
--------------
            1
            2
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): IS DOCUMENT tests an XML value, not a value of INTEGER [ISO/IEC 9075-14:2006 draft §8.3 SR1] (at character 27)
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
