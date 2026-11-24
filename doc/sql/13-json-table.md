# JSON_TABLE

[Contents](README.md)

JSON_TABLE presents JSON text as a table: a row for each item a path
finds, and a column for each value taken of the item by a path of its
own. It is a table reference of the FROM clause, read laterally, so its
text may be a column of a table before it. Nested columns turn arrays
within each item into rows of their own, joined to their parent's row
as a plan says. JSON_VALUE, JSON_QUERY and the path language are in
[Value Expressions](04-expressions.md#sqljson); the examples here read
their table PURCHASES.

## Syntax

```
JSON_TABLE ( <context> [ FORMAT JSON ] , '<path>' [ AS <path name> ]
             [ PASSING <value> [ FORMAT JSON ] AS <identifier> [ , ... ] ]
             COLUMNS ( <column definition> [ , ... ] )
             [ PLAN ( <plan> ) | PLAN DEFAULT ( <choices> ) ]
             [ { ERROR | EMPTY } ON ERROR ] )
  [ AS ] <correlation name> [ ( <derived column list> ) ]

<column definition> ::=
    <column name> FOR ORDINALITY
  | <column name> <data type> [ PATH '<path>' ]
        [ { ERROR | NULL | DEFAULT <value expression> } ON EMPTY ]
        [ { ERROR | NULL | DEFAULT <value expression> } ON ERROR ]
  | <column name> <data type> FORMAT JSON [ PATH '<path>' ]
        [ WITHOUT [ ARRAY ] WRAPPER | WITH [ CONDITIONAL | UNCONDITIONAL ] [ ARRAY ] WRAPPER ]
        [ { KEEP | OMIT } QUOTES [ ON SCALAR STRING ] ]
        [ { ERROR | NULL | EMPTY ARRAY | EMPTY OBJECT } ON EMPTY ]
        [ { ERROR | NULL | EMPTY ARRAY | EMPTY OBJECT } ON ERROR ]
  | NESTED [ PATH ] '<path>' [ AS <path name> ] COLUMNS ( <column definition> [ , ... ] )

<plan> ::=
    <path name>
  | <path name> { OUTER | INNER } <plan primary>
  | <plan primary> UNION <plan primary> [ UNION <plan primary> ]...
  | <plan primary> CROSS <plan primary> [ CROSS <plan primary> ]...

<plan primary> ::= <path name> | ( <plan> )

<choices> ::= { INNER | OUTER } [ , { UNION | CROSS } ] | { UNION | CROSS } [ , { INNER | OUTER } ]
```

The context is a character string of JSON text, or a value of the JSON
type. Each path is a character string literal, read as the statement is
prepared.

## Columns

The path after the context is the **row pattern**: each item it finds
is a row. Each column takes its value of that item:

| Column | Value |
|---|---|
| `name FOR ORDINALITY` | the row's number, from 1, an INTEGER |
| `name type [PATH ...]` | the one scalar its path finds, cast to its type, as JSON_VALUE gives it: a character string, numeric, BOOLEAN or datetime type |
| `name type FORMAT JSON [PATH ...]` | what its path finds, as JSON text of its type, a character string type, as JSON_QUERY gives it, with its wrapper and quotes |
| `name JSON [PATH ...]` | the same, a value of the JSON type |

A column without PATH reads the member of its name: `$."NAME"`, the
name as SQL folds it, so a column `sku` reads the member `SKU`, and a
column `"sku"` the member `sku`.

```sql
SELECT P.ID, JT.*
  FROM PURCHASES P,
       JSON_TABLE(P.DOC, 'lax $.items[*]'
         COLUMNS (N FOR ORDINALITY, "sku" VARCHAR(5), QTY INTEGER PATH 'lax $.qty',
                  ITEM VARCHAR(30) FORMAT JSON PATH 'lax $')) AS JT
  ORDER BY P.ID, N;
```

```
           ID |            N | sku   |          QTY | ITEM
--------------+--------------+-------+--------------+--------------------------------
            1 |            1 | A1    |            2 | {"sku":"A1","qty":2}
            1 |            2 | B7    |            1 | {"sku":"B7","qty":1}
(2 rows)
```

PURCHASES 2 has no items, so it has no rows. Written as the second
operand of a LEFT join, a JSON table keeps such a row, its columns null:

```sql
SELECT P.ID, JT.SKU
  FROM PURCHASES P LEFT JOIN
       JSON_TABLE(P.DOC, 'lax $.items[*]' COLUMNS (SKU VARCHAR(5) PATH 'lax $.sku')) AS JT ON TRUE
  ORDER BY P.ID;
```

```
           ID | SKU
--------------+-------
            1 | A1
            1 | B7
            2 | NULL
(3 rows)
```

## ON EMPTY and ON ERROR

A column's behavior ON EMPTY says what its value is when its path finds
nothing, and ON ERROR what it is in place of a data exception, an
SQLSTATE of class 22, as of a cast that fails or of two items found. A
regular column takes ERROR, NULL or DEFAULT and a value expression,
which may read the columns of the tables before the JSON table and is
evaluated only when it is the value. A formatted column takes ERROR,
NULL, EMPTY ARRAY or EMPTY OBJECT. Without one, ON EMPTY is NULL, and ON
ERROR is NULL, or ERROR if the table says ERROR ON ERROR.

The table's own `ERROR ON ERROR` or `EMPTY ON ERROR`, EMPTY if not
written, says what a data exception of a row pattern does, as of text
that is no JSON text: EMPTY makes that pattern's table empty, ERROR
raises it.

```sql
SELECT P.ID, JT.*
  FROM PURCHASES P,
       JSON_TABLE(P.DOC, 'lax $'
         COLUMNS (BUYER VARCHAR(10) PATH 'lax $.buyer',
                  NOTE VARCHAR(10) PATH 'lax $.note' DEFAULT 'none' ON EMPTY,
                  FIRST_QTY INTEGER PATH 'lax $.items[0].qty' DEFAULT -P.ID ON EMPTY,
                  QTYS INTEGER PATH 'lax $.items[*].qty' NULL ON ERROR)) AS JT
  ORDER BY P.ID;
SELECT * FROM JSON_TABLE('{"a":', 'lax $' COLUMNS (A INTEGER)) AS JT;
SELECT * FROM JSON_TABLE('{"a":', 'lax $' COLUMNS (A INTEGER) ERROR ON ERROR) AS JT;
```

```
           ID | BUYER      | NOTE       |    FIRST_QTY |         QTYS
--------------+------------+------------+--------------+--------------
            1 | Ada        | none       |            2 |         NULL
            2 | Grace      | NULL       |           -2 |         NULL
(2 rows)
            A
--------------
(0 rows)
turborel: SQLCODE -301 (SQLSTATE 22032): the string is not a JSON text: a value is missing (at character 6) [ISO/IEC 9075-2:2023 §9.42 GR3a iii 1]
```

Ada's document has no member `note`, so its NOTE is the DEFAULT, where
Grace's has the JSON null, which is NULL. Grace has no first item, so
FIRST_QTY is the DEFAULT, of her row's ID. QTYS finds two numbers in
Ada's items, an error ON ERROR makes NULL.

## Nested columns and plans

`NESTED PATH` gives the rows of its own path of each row of the columns
it is nested in, and nests to any depth. Its rows are joined to their
parent's row: by default each parent's row is kept, null extended if it
has none (OUTER), and the rows of siblings, two NESTED clauses side by
side, follow one another, each with the other's columns null (UNION).

`PLAN DEFAULT (INNER)` drops a parent's row of no nested rows, and
`PLAN DEFAULT (CROSS)` joins each row of a sibling with each of the
other's. `PLAN ( ... )` says how each pair is joined, every path named
once: then the row pattern and each NESTED clause are named by AS. A
plan joins a path by OUTER or INNER to the path its columns are nested
in, so `PLAN (R OUTER (I UNION T))` is the default here.

```sql
SELECT JT.*
  FROM PURCHASES P,
       JSON_TABLE(P.DOC, 'lax $' AS R
         COLUMNS (BUYER VARCHAR(10) PATH 'lax $.buyer',
                  NESTED PATH 'lax $.items[*]' AS I COLUMNS (SKU VARCHAR(5) PATH 'lax $.sku'),
                  NESTED PATH 'lax $.tags[*]' AS T COLUMNS (TAG VARCHAR(10) PATH 'lax $'))) AS JT
  ORDER BY BUYER, SKU;
SELECT JT.*
  FROM JSON_TABLE('{"buyer":"Ada","items":[{"sku":"A1"},{"sku":"B7"}],"tags":["new","gift"]}', 'lax $' AS R
         COLUMNS (BUYER VARCHAR(10) PATH 'lax $.buyer',
                  NESTED PATH 'lax $.items[*]' AS I COLUMNS (SKU VARCHAR(5) PATH 'lax $.sku'),
                  NESTED PATH 'lax $.tags[*]' AS T COLUMNS (TAG VARCHAR(10) PATH 'lax $'))
         PLAN (R INNER (I CROSS T))) AS JT;
```

```
 BUYER      | SKU   | TAG
------------+-------+------------
 Ada        | A1    | NULL
 Ada        | B7    | NULL
 Grace      | NULL  | NULL
(3 rows)
 BUYER      | SKU   | TAG
------------+-------+------------
 Ada        | A1    | new
 Ada        | A1    | gift
 Ada        | B7    | new
 Ada        | B7    | gift
(4 rows)
```

## PASSING

The PASSING arguments are variables of the row pattern and of the
nested paths, `$` and the name, a regular identifier's in upper case.
A column's path names none.

```sql
SELECT P.ID, JT.SKU
  FROM PURCHASES P,
       JSON_TABLE(P.DOC, 'lax $.items[*] ? (@.qty >= $LEAST)' PASSING 2 AS LEAST
         COLUMNS (SKU VARCHAR(5) PATH 'lax $.sku')) AS JT;
```

```
           ID | SKU
--------------+-------
            1 | A1
(1 row)
```

## EXPLAIN

EXPLAIN *(extension)* shows a JSON table by its row pattern, the number of its
columns, and its plan, the default one spelled out, a path of no name
by its text.

```sql
EXPLAIN SELECT JT.* FROM PURCHASES P,
  JSON_TABLE(P.DOC, 'lax $' AS R COLUMNS (B VARCHAR(10) PATH 'lax $.buyer',
    NESTED 'lax $.items[*]' COLUMNS (S VARCHAR(5) PATH 'lax $.sku'))) AS JT;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Project JT.B, JT.S
   JSON_TABLE 'lax $' of 2 columns PLAN (R OUTER ('lax $.items[*]')) of P.DOC
     Scan ANN.PURCHASES
 -- rules: none
 -- optimized:
 Project JT.B, JT.S
   JSON_TABLE 'lax $' of 2 columns PLAN (R OUTER ('lax $.items[*]')) of P.DOC
     Scan ANN.PURCHASES
 -- physical:
 Project JT.B, JT.S
   JSON_TABLE 'lax $' of 2 columns PLAN (R OUTER ('lax $.items[*]'))
     SeqScan ANN.PURCHASES  (rows=2, cost=1.8)
(13 rows)
```

## Errors

| SQLSTATE | SQLCODE | Cause |
|---|---|---|
| 22032 | -301 | ERROR ON ERROR, of a context item that is no JSON text, or of a parent's item of a NESTED clause that has none, a datetime |
| 2203A and the path language's other SQLSTATEs | -301 | ERROR ON ERROR, of a row pattern in strict mode that finds no member or element |
| 22034, 22035, 2203F, 2203G, 22018 | -301 | of a column ERROR ON EMPTY or ERROR ON ERROR, as of JSON_VALUE |
| 22001 | -301 | of a formatted column whose text is longer than its type |
| 42000 | -103 | a Syntax Rule broken: a column of a type it does not take, a formatted column of a character string type without FORMAT JSON, a quotes behavior with a wrapper WITH, a column path that names a variable, a path name or column name written twice, a PLAN that names a path not once or joins it to a path it is not nested in, NESTED COLUMNS of no name with a PLAN, or JSON_TABLE in a CHECK constraint |
| 42000 | -102 | DEFAULT as a formatted column's behavior |

## Standard

JSON_TABLE is ISO/IEC 9075-2:2023 §7.11, Features T821, T824 (PLAN),
T825 and T826 (ON EMPTY and ON ERROR), T827 (sibling NESTED COLUMNS) and
T838 (PLAN DEFAULT), and T823 (PASSING). A path name not written is
named by its row pattern in EXPLAIN alone.
