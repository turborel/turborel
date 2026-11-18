# The JSON Type

[Contents](README.md)

JSON is a data type of its own. A value of it is a JSON document held
parsed, so a query reads it without parsing its text again. A column,
a variable and a parameter can be of type JSON. JSON text, a character
string, is its input and output form. The SQL/JSON functions of
[Value Expressions](04-expressions.md#sqljson) take a value of JSON
wherever they take JSON text, and the simplified accessor reads members
and elements with the period and brackets of the path language.

## Syntax

```
<JSON type> ::= JSON

JSON ( <string value expression> [ FORMAT JSON ]
       [ { WITH | WITHOUT } UNIQUE [ KEYS ] ] )
JSON_SCALAR ( <value expression> )
JSON_SERIALIZE ( <JSON value expression> [ RETURNING <data type> [ FORMAT JSON ] ] )

<JSON simplified accessor> ::= <value expression primary> <op>...
<op> ::= . <key> | . * | [ <subscript> [ , ... ] ] | [ * ] | . <method> ( [ <argument> [ , ... ] ] )
```

## Values

A value of JSON is an SQL/JSON item: an object, an array, a string, a
number, true, false, or null. The JSON null is a value, not the SQL
null value, so a column of JSON can hold both. An object keeps its
members in the order they were written, and may hold two members of one
key unless it was parsed WITH UNIQUE KEYS.

JSON ( ) parses a character string's JSON text into a value. Without a
key uniqueness clause it is WITHOUT UNIQUE KEYS. FORMAT JSON may be
written, and means what is implicit. JSON_SCALAR makes a string, number,
truth value or datetime a scalar of JSON, and keeps the SQL null value
null. JSON_SERIALIZE writes a value's JSON text, as a CHARACTER
VARYING(32767) or the character string type RETURNING names.

A character string stored in a column of JSON is parsed as JSON ( )
parses it *(extension)*. So is a string CAST to JSON, and CAST of JSON to
a character string gives its JSON text *(extension)*. No other type is
cast to or from JSON. Values brought together, by UNION or the rows of
VALUES, are of JSON if all are, and JSON with another type is an error.

```sql
CREATE TABLE ORDERS (ID INTEGER PRIMARY KEY, DOC JSON);
INSERT INTO ORDERS VALUES
  (1, JSON('{"buyer":"Ada","items":[{"sku":"A1","qty":2},{"sku":"B7","qty":1}],"paid":true}')),
  (3, JSON_SCALAR('pending')),
  (4, NULL);
INSERT INTO ORDERS VALUES (2, '{"buyer":"Bob","items":[],"paid":false}');
SELECT ID, DOC FROM ORDERS ORDER BY ID;
VALUES (JSON('{"a":1,"a":2}'), CAST('[1, 2.50, null]' AS JSON), JSON_SCALAR(12.5));
VALUES JSON('{"a":1,"a":2}' WITH UNIQUE KEYS);
SELECT JSON_SERIALIZE(DOC RETURNING VARCHAR(40)) FROM ORDERS WHERE ID = 2;
VALUES CAST(1 AS JSON);
```

```
OK
OK
OK
           ID | DOC
--------------+--------------------------------------------------------------
            1 | {"buyer":"Ada","items":[{"sku":"A1","qty":2},{"sku":"B7","qty":1}],"paid":true}
            2 | {"buyer":"Bob","items":[],"paid":false}
            3 | "pending"
            4 | NULL
(4 rows)
                                                              |                                                              |
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 {"a":1,"a":2}                                                | [1,2.50,null]                                                | 12.5
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22037): two members of a JSON object have the key "a" [ISO/IEC 9075-2:2023 §9.42 GR3a iii 2]

------------------------------------------
 {"buyer":"Bob","items":[],"paid":false}
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): CAST of NUMERIC(1,0) to JSON is not valid [ISO/IEC 9075-2:2023 §6.13 SR5, SR6, SR7, SR8, SR12] (at character 7)
```

## Reading values

JSON_VALUE, JSON_QUERY, JSON_EXISTS, IS JSON and JSON_TABLE take a value
of JSON as they take JSON text, FORMAT JSON implicit. A path of members,
wildcards and integer subscripts in lax mode is read on the value as it
is stored, from the root to what it selects. IS JSON of a value of JSON
does not parse it: it is TRUE of its kind, WITH UNIQUE KEYS too.

JSON_QUERY of a value of JSON returns JSON unless RETURNING names a
character string type, and so do JSON_OBJECT, JSON_ARRAY,
JSON_OBJECTAGG and JSON_ARRAYAGG of one. RETURNING JSON may be written
of each. A value of JSON put in an object or array is its item, not a
string of its text.

```sql
SELECT ID, JSON_VALUE(DOC, 'lax $.buyer') AS BUYER,
       JSON_QUERY(DOC, 'lax $.items[*].sku' WITH ARRAY WRAPPER) AS SKUS,
       DOC IS JSON OBJECT AS OBJ
  FROM ORDERS ORDER BY ID;
SELECT JSON_OBJECT('id': ID, 'doc': DOC) FROM ORDERS WHERE ID = 3;
```

```
           ID | BUYER                                                        | SKUS                                                         | OBJ
--------------+--------------------------------------------------------------+--------------------------------------------------------------+-------
            1 | Ada                                                          | ["A1","B7"]                                                  | TRUE
            2 | Bob                                                          | NULL                                                         | TRUE
            3 | NULL                                                         | NULL                                                         | FALSE
            4 | NULL                                                         | NULL                                                         | NULL
(4 rows)

--------------------------------------------------------------
 {"id":3,"doc":"pending"}
(1 row)
```

## The simplified accessor

A period and a name after a value of JSON reads a member, brackets read
elements, and a method after a period applies an item method of the path
language. The op chain is read as the path `lax $` followed by it, as
JSON_QUERY reads it WITH CONDITIONAL ARRAY WRAPPER, NULL ON EMPTY and
NULL ON ERROR, a value of JSON. A last method that gives an SQL type,
`size()`, `double()`, `string()` and the other casts, is read as
JSON_VALUE RETURNING that type instead.

A name after a column is an identifier, so a regular one is in upper
case: write `DOC."buyer"` for the key buyer. A quoted string, `.*`,
`[*]`, `last` and `to` are read as the path language writes them, keys
in the case written. A `.*` that ends an item of a select list, before
a comma, AS or FROM, is SQL's all fields reference of a row instead;
write the chain on after it, or start it with a quoted key, to read a
document's members there.

```sql
SELECT ID, DOC."buyer", DOC."items"[0]."sku", DOC."items".size() AS N
  FROM ORDERS WHERE ID < 3 ORDER BY ID;
SELECT DOC.'items'[*].'qty', DOC.'items'[last] FROM ORDERS WHERE ID = 1;
```

```
           ID | buyer                                                        |                                                              |                    N
--------------+--------------------------------------------------------------+--------------------------------------------------------------+----------------------
            1 | "Ada"                                                        | "A1"                                                         |                    2
            2 | "Bob"                                                        | NULL                                                         |                    0
(2 rows)
                                                              |
--------------------------------------------------------------+--------------------------------------------------------------
 [2,1]                                                        | {"sku":"B7","qty":1}
(1 row)
```

## Comparison and ordering

Any two values of JSON compare, and a value of JSON compares with no
other type. Equal values are equivalent items: numbers of one value,
strings of the same characters, arrays of equal elements in order, and
objects of equal members whatever their order. They order the JSON null
first, then numbers by value, strings by character, false, true,
datetimes, arrays element by element, and objects by their members
sorted. So GROUP BY, DISTINCT, ORDER BY, UNION and keys take JSON
columns.

```sql
VALUES (JSON('{"a":1,"b":[2]}') = JSON('{"b":[2.0],"a":1.00}'),
        JSON('"a"') < JSON('"a "'), JSON('2') < JSON('"1"'));
SELECT PAID, COUNT(*) FROM (SELECT DOC."paid" AS PAID FROM ORDERS) AS P
  GROUP BY PAID ORDER BY 1;
```

```
       |       |
-------+-------+-------
 TRUE  | TRUE  | TRUE
(1 row)
 PAID                                                         |
--------------------------------------------------------------+----------------------
 false                                                        |                    1
 true                                                         |                    1
 NULL                                                         |                    2
(3 rows)
```

## Errors

| SQLSTATE | SQLCODE | Cause |
|---|---|---|
| 22032 | -301 | JSON ( ), CAST or a stored string of no JSON text; JSON_SERIALIZE or CAST to a string of a datetime scalar, which no JSON text holds |
| 22037 | -301 | JSON ( ) WITH UNIQUE KEYS of an object with two members of one key |
| 22001 | -301 | JSON_SERIALIZE of text longer than its type |
| 2A000 | -103 | CAST of JSON to or from another type than a character string; a comparison of JSON with another type, or JSON brought together with it; JSON_SCALAR of a type it does not take; JSON_SERIALIZE of no JSON value or RETURNING JSON; JSON_QUERY RETURNING JSON with OMIT QUOTES |

## Standard

The JSON type is ISO/IEC 9075-2:2023 §4.8, Features T801 and T802, with
JSON ( ) §6.40, JSON_SCALAR §6.41, JSON_SERIALIZE §6.37 and the
simplified accessor §6.36, Features T860 to T864. FORMAT JSON written
where it is implicit is T851. Comparison, grouping and ordering are
§4.8.2 and §8.2, Features T879 to T882. CAST between JSON and character
strings, and a string stored as JSON, are extensions.
