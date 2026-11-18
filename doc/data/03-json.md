# JSON Data

[Specialized Data](README.md)

A shop keeps each order as the JSON document its web store sends, and
needs its customers, items and totals in SQL. This guide stores the
documents, checks them as they are inserted, reads values and parts out
of them with JSON_VALUE and JSON_QUERY, finds orders with JSON_EXISTS,
turns their items into rows with JSON_TABLE, and builds JSON for a
reply with JSON_OBJECT and JSON_ARRAYAGG. The
[reference](../sql/04-expressions.md#sqljson) gives every rule, and
[The SQL/JSON Path Language](../sql/11-json-path.md) the paths.

## The orders

A document is a character string. A CHECK constraint of IS JSON keeps
out text that is no JSON object, or an object of one key twice:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE ORDERS (ID INTEGER PRIMARY KEY,
    DOC VARCHAR(2000) CHECK (DOC IS JSON OBJECT WITH UNIQUE KEYS));
INSERT INTO ORDERS VALUES
    (1, '{"customer":"Ada","placed":"2024-03-01 09:30:00","items":[{"sku":"PEN","qty":3,"price":1.20},{"sku":"INK","qty":1,"price":7.50}]}'),
    (2, '{"customer":"Grace","placed":"2024-03-02 14:05:00","items":[{"sku":"PAD","qty":2,"price":3.00}],"gift":true}'),
    (3, '{"customer":"Alan","placed":"2024-03-02 18:45:00","items":[]}');
COMMIT;
INSERT INTO ORDERS VALUES (4, '{"customer":"Bob","customer":"Rob"}');
INSERT INTO ORDERS VALUES (5, '[1,2]');
```

```
OK
OK
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.ORDERS_CHECK1 of ANN.ORDERS is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.ORDERS_CHECK1 of ANN.ORDERS is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
```

## Reading values

JSON_VALUE takes one scalar out of a document, cast to the type
RETURNING names. JSON_QUERY takes an array or object, as JSON text.
A path that finds nothing gives the null value, unless ON EMPTY says
otherwise:

```sql
SELECT ID, JSON_VALUE(DOC, 'lax $.customer' RETURNING VARCHAR(10)) AS CUSTOMER,
       JSON_VALUE(DOC, 'lax $.items.size()' RETURNING INTEGER) AS LINES,
       JSON_VALUE(DOC, 'lax $.gift' RETURNING BOOLEAN DEFAULT FALSE ON EMPTY) AS GIFT,
       JSON_QUERY(DOC, 'lax $.items[0]' RETURNING VARCHAR(40)) AS FIRST_ITEM
  FROM ORDERS ORDER BY ID;
```

```
           ID | CUSTOMER   |        LINES | GIFT  | FIRST_ITEM
--------------+------------+--------------+-------+------------------------------------------
            1 | Ada        |            2 | FALSE | {"sku":"PEN","qty":3,"price":1.20}
            2 | Grace      |            1 | TRUE  | {"sku":"PAD","qty":2,"price":3.00}
            3 | Alan       |            0 | FALSE | NULL
(3 rows)
```

A DEFAULT is evaluated only for a row that takes it, so a DEFAULT that
could fail, as a division, fails only where it is used.

## Finding orders

JSON_EXISTS is true of a document in which the path finds an item. A
filter finds the items a condition holds of, and PASSING gives the path
values from SQL:

```sql
SELECT ID FROM ORDERS WHERE JSON_EXISTS(DOC, 'lax $.items[*] ? (@.sku == $SKU)' PASSING 'PAD' AS SKU);
SELECT ID FROM ORDERS WHERE JSON_EXISTS(DOC, 'lax $.items[*] ? (@.qty * @.price > 5)') ORDER BY ID;
```

```
           ID
--------------
            2
(1 row)
           ID
--------------
            1
            2
(2 rows)
```

## Items as rows

JSON_TABLE turns the items of each order into rows, which SQL can then
group and sum:

```sql
SELECT O.ID, I.SKU, I.QTY * I.PRICE AS AMOUNT
  FROM ORDERS AS O, JSON_TABLE(O.DOC, 'lax $.items[*]'
       COLUMNS (SKU VARCHAR(5) PATH 'lax $.sku', QTY INTEGER PATH 'lax $.qty', PRICE DECIMAL(6,2) PATH 'lax $.price')) AS I
  ORDER BY O.ID, I.SKU;
SELECT O.ID, SUM(I.QTY * I.PRICE) AS TOTAL
  FROM ORDERS AS O, JSON_TABLE(O.DOC, 'lax $.items[*]'
       COLUMNS (QTY INTEGER PATH 'lax $.qty', PRICE DECIMAL(6,2) PATH 'lax $.price')) AS I
  GROUP BY O.ID ORDER BY O.ID;
```

```
           ID | SKU   |               AMOUNT
--------------+-------+----------------------
            1 | INK   |                 7.50
            1 | PEN   |                 3.60
            2 | PAD   |                 6.00
(3 rows)
           ID |                TOTAL
--------------+----------------------
            1 |                11.10
            2 |                 6.00
(2 rows)
```

## Dates and times

JSON has no datetime type, so a document holds a date or time as a
string. JSON_VALUE RETURNING a datetime type casts the string as CAST
does, which reads a literal's form, with a space between the date and
the time:

```sql
SELECT ID, JSON_VALUE(DOC, 'lax $.placed' RETURNING TIMESTAMP(0)) AS PLACED
  FROM ORDERS WHERE CAST(JSON_VALUE(DOC, 'lax $.placed' RETURNING TIMESTAMP) AS DATE) = DATE '2024-03-02' ORDER BY ID;
```

```
           ID | PLACED
--------------+---------------------
            2 | 2024-03-02 14:05:00
            3 | 2024-03-02 18:45:00
(2 rows)
```

The ISO 8601 form many web stores send has a `T` between the date and
the time. That is no literal's form, so the cast is SQLSTATE 22018, and
no datetime template has a field or delimiter for the letter, so the
path's `datetime()` does not read it either (22031). Take the string,
put a space in place of the `T` with OVERLAY, and cast that:

```sql
VALUES JSON_VALUE('"2024-03-02T14:05:00"', 'lax $' RETURNING TIMESTAMP ERROR ON ERROR);
VALUES CAST(OVERLAY(JSON_VALUE('"2024-03-02T14:05:00"', 'lax $' RETURNING VARCHAR(19)) PLACING ' ' FROM 11) AS TIMESTAMP);
```

```
turborel: SQLCODE -301 (SQLSTATE 22018): '2024-03-02T14:05:00' is no TIMESTAMP(6) literal's string [ISO/IEC 9075:1992 §6.10 GR9a ii, GR10a ii, GR11a ii]

----------------------------
 2024-03-02 14:05:00.000000
(1 row)
```

## Building JSON

JSON_OBJECT and JSON_ARRAY make JSON text of SQL values, and
JSON_OBJECTAGG and JSON_ARRAYAGG of a group's rows. A constructor
inside another is JSON, not a string, so they nest:

```sql
SELECT O.ID, JSON_OBJECT('order' VALUE O.ID,
                         'skus' VALUE JSON_ARRAYAGG(I.SKU ORDER BY I.SKU)
                         RETURNING VARCHAR(60)) AS SUMMARY
  FROM ORDERS AS O, JSON_TABLE(O.DOC, 'lax $.items[*]' COLUMNS (SKU VARCHAR(5) PATH 'lax $.sku')) AS I
  GROUP BY O.ID ORDER BY ID;
```

```
           ID | SUMMARY
--------------+--------------------------------------------------------------
            1 | {"order":1,"skus":["INK","PEN"]}
            2 | {"order":2,"skus":["PAD"]}
(2 rows)
```

Without RETURNING, JSON text is CHARACTER VARYING(32767), and a longer
text is SQLSTATE 22001. RETURNING CLOB holds a text of any length, as
of a large export of many rows:

```sql
VALUES CHAR_LENGTH(JSON_ARRAY(RPAD('x', 40000, 'x') RETURNING CLOB));
VALUES CHAR_LENGTH(JSON_ARRAY(RPAD('x', 40000, 'x')));
```

```

----------------------
                32771
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22001): the JSON text of 32771 characters or octets is longer than CHARACTER VARYING(32767) [ISO/IEC 9075-2:2023 §9.43 GR2a iii]
```

## Indexes

A json index *(extension)* finds the rows a JSON_EXISTS, a JSON_VALUE
compared equal to a value, or a JSON_CONTAINS *(extension)* holds of,
without reading every document
([CREATE INDEX](../sql/statements/create-index.md)). It holds the
paths and values of a column's documents, a character string's or the
JSON type's. A path is the chain of member names from the root, array
steps dropped, so `$.items[0].sku` and `$.items.sku` are one path:

```sql
COMMIT;
CREATE INDEX ORDERS_DOC ON ORDERS (DOC) USING json;
SELECT ID FROM ORDERS WHERE JSON_VALUE(DOC, 'lax $.customer') = 'Grace';
```

```
OK
OK
           ID
--------------
            2
(1 row)
```

The planner looks a call up when its cost says so, of a call of the
indexed column whose other arguments are constants or parameters and
that gives null or false on an error, as JSON_EXISTS does by default;
never of ERROR ON ERROR. A filter of `==` with a literal or a PASSING
variable, joined by `&&` and `||`, asks for its values; a wildcard, a
method or another comparison asks for its path alone. The index may so
find more documents than the call holds of, and each row it finds is
checked by the whole WHERE clause, so it changes no result. A document
that is no JSON text is found by every lookup, so the WHERE clause
raises its error as a scan would. EXPLAIN shows the call looked up, as
the condition of an index scan of ORDERS_DOC. Of three orders, a scan
costs less, and the plan reads the table:

```sql
EXPLAIN SELECT ID FROM ORDERS WHERE JSON_EXISTS(DOC, 'lax $.items[*] ? (@.sku == $SKU)' PASSING 'PAD' AS SKU);
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Project ORDERS.ID
   Filter JSON_EXISTS 'lax $.items[*] ? (@.sku == $SKU)' PASSING AS "SKU" FALSE ON ERROR(ORDERS.DOC, 'PAD')
     Scan ANN.ORDERS
 -- rules: none
 -- optimized:
 Project ORDERS.ID
   Filter JSON_EXISTS 'lax $.items[*] ? (@.sku == $SKU)' PASSING AS "SKU" FALSE ON ERROR(ORDERS.DOC, 'PAD')
     Scan ANN.ORDERS
 -- physical:
 Project ORDERS.ID
   Filter JSON_EXISTS 'lax $.items[*] ? (@.sku == $SKU)' PASSING AS "SKU" FALSE ON ERROR(ORDERS.DOC, 'PAD')  (est. rows=1)
     SeqScan ANN.ORDERS  (rows=3, cost=3.0)
(13 rows)
```

JSON_CONTAINS(document, candidate) is true of a document containing the
candidate: each member of an object in the member of the same key, each
element of an array in some element, a scalar equal to it. The index
answers it too:

```sql
SELECT ID FROM ORDERS WHERE JSON_CONTAINS(DOC, '{"items":[{"sku":"INK"}]}');
```

```
           ID
--------------
            1
(1 row)
```

An index statement runs outside a transaction, so COMMIT ends the one
the queries began. JSON.PATHS_ONLY = 'ON' keeps each document's paths and no values, a
smaller index whose lookups of a value find every document holding its
path. JSON.MAX_DEPTH, 0 to 128, is how many member names of a path it
keeps; a deeper path is found by its first names. A change of either
makes the index anew:

```sql
COMMIT;
ALTER INDEX ORDERS_DOC SET (JSON.PATHS_ONLY = 'ON', JSON.MAX_DEPTH = 1);
SELECT ID FROM ORDERS WHERE JSON_EXISTS(DOC, 'lax $.gift');
```

```
OK
OK
           ID
--------------
            2
(1 row)
```
