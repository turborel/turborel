# The SQL/JSON Path Language

[Contents](README.md)

An SQL/JSON path expression says which items of JSON text JSON_VALUE,
JSON_QUERY, JSON_EXISTS and JSON_TABLE take
([Value Expressions](04-expressions.md#sqljson)). It is a character
string literal, parsed as the statement is prepared, so a path that is
none is refused then. Unlike SQL, the language is case-sensitive:
`$.Name` is not `$.name`, and the key words `lax`, `strict`, `true`,
`false`, `null`, `last` and the methods are lower case.

The examples on this page are run in order by the user ANN.

## Grammar

```
path        ::= { lax | strict } wff
wff         ::= additive
additive    ::= multiplicative [ { + | - } multiplicative ]...
multiplicative ::= unary [ { * | / | % } unary ]...
unary       ::= [ + | - ] accessed
accessed    ::= primary [ accessor ]...
primary     ::= $ | @ | $name | last | literal | ( wff )
accessor    ::= .key | ."key" | .* | [ subscript, ... ] | [*]
              | .method ( [ arguments ] ) | ? ( predicate )
subscript   ::= wff [ to wff ]
literal     ::= number | "string" | true | false | null
predicate   ::= disjunct [ || disjunct ]...
disjunct    ::= conjunct [ && conjunct ]...
conjunct    ::= ! ( predicate ) | ( predicate ) is unknown | ( predicate )
              | wff { == | != | <> | < | <= | > | >= } wff
              | exists ( wff ) | wff like_regex "pattern" [ flag "flags" ]
              | wff starts with { "string" | $name }
```

Accessors bind tightest, then a sign, then `*`, `/` and `%`, then `+`
and `-`; in a predicate, `!` before `&&` before `||`. So `-$.a` is the
negation of `$.a`, and `1 + 2 * 3` is 7. `$` is the JSON text, `@` the
item a filter tests, `$name` a variable of PASSING, and `last` the last
subscript of the array a subscript is of. A number is written as in
JSON, or in hexadecimal, `0x1F`; a string in double quotation marks,
with JSON's escapes.

```sql
VALUES (JSON_VALUE('{"a":2}', 'lax -$.a + 1 * 3' RETURNING INTEGER), JSON_VALUE('[]', 'lax 0x1F' RETURNING INTEGER),
        JSON_VALUE('{"a b":"x"}', 'lax $."a b"'));
```

```
              |              |
--------------+--------------+--------------------------------------------------------------
            1 |           31 | x
(1 row)
```

## Lax and strict

The mode says what happens where the text is not shaped as the path
expects. In lax mode an array is taken for its elements where a member
accessor or a method wants one item, a single item is taken for an array
of one where an array accessor wants an array, and a member or element
that is not there is no item. In strict mode each is an exception
condition, which ON ERROR then replaces.

```sql
VALUES (JSON_QUERY('[{"a":1},{"a":2}]', 'lax $.a' WITH WRAPPER), JSON_VALUE('{"a":5}', 'lax $[0].a'),
        JSON_VALUE('{}', 'lax $.b'));
VALUES JSON_VALUE('[{"a":1}]', 'strict $.a' ERROR ON ERROR);
VALUES JSON_VALUE('{"a":5}', 'strict $[0]' ERROR ON ERROR);
VALUES JSON_VALUE('[1]', 'strict $[3]' ERROR ON ERROR);
```

```
                                                              |                                                              |
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 [1,2]                                                        | 5                                                            | NULL
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2203A): an SQL/JSON item is not an object with a member of the key a [ISO/IEC 9075-2:2023 §9.46 GR11g ii 1 B II]
turborel: SQLCODE -301 (SQLSTATE 22039): an SQL/JSON item is not an array [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 B]
turborel: SQLCODE -301 (SQLSTATE 22033): an SQL/JSON subscript is outside its array, or the first of a range past the last [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 F II 2 a]
```

## Accessors

| Accessor | Gives |
|---|---|
| `.key`, `."key"` | the value of the member of that key of each object |
| `.*` | the value of every member of each object, in order |
| `[n]`, `[n to m]`, `[n, m]` | the elements of those subscripts of each array, counted from 0 |
| `[last]`, `[last - 1]` | the last element, and the one before it |
| `[*]` | every element of each array |
| `? (predicate)` | each item the predicate is true of |

A subscript is a number, made a whole number by truncation, or an
expression of one; `n to m` is every subscript from n to m. In lax mode a
subscript outside the array gives nothing; in strict mode it is SQLSTATE
22033, as is a subscript that is no number in either mode.

```sql
VALUES (JSON_QUERY('[10,20,30,40]', 'lax $[1 to last]' WITH WRAPPER), JSON_QUERY('[10,20,30,40]', 'lax $[0, last - 1]' WITH WRAPPER),
        JSON_QUERY('{"a":1,"b":[2]}', 'lax $.*' WITH WRAPPER));
```

```
                                                              |                                                              |
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 [20,30,40]                                                   | [10,30]                                                      | [1,[2]]
(1 row)
```

## Filters

A filter, `? (predicate)`, keeps each item its predicate is true of; in
the predicate `@` is that item. A predicate is true, false or unknown.
An exception condition within it, as an operand that raises one, makes
it unknown, so the item is not kept and the condition is not raised.
`==`, `!=`, `<`, `<=`, `>` and `>=` compare items of a kind. Numbers
compare by value, strings by their characters' code points, and truth
values and datetimes as SQL compares them. `<>` is `!=`. JSON's `null` equals `null` and is unequal to
every other item. A comparison of items of two other kinds, or of an
array or object, is unknown. Of operands of several items, a comparison
is true if any pair compares true; in strict mode it is unknown if any
pair cannot be compared.

`exists (wff)` is true if the expression gives an item. `starts with`
is true of a string that begins with the string, or the variable's
string, after it. `(predicate) is unknown` is true if the predicate is
unknown. `!`, `&&` and `||` are not, and, or, of three values.

```sql
VALUES (JSON_QUERY('[1,"2",3,null]', 'lax $[*] ? (@ > 1)' WITH WRAPPER),
        JSON_QUERY('[{"n":"Ada"},{"n":"Alan"},{"m":1}]', 'lax $[*] ? (@.n starts with "Al").n' WITH WRAPPER),
        JSON_QUERY('[1,"x",3]', 'lax $[*] ? ((@ > 2) is unknown)' WITH WRAPPER),
        JSON_QUERY('[{"a":1},{"b":2}]', 'lax $[*] ? (exists (@.a))' WITH WRAPPER));
```

```
                                                              |                                                              |                                                              |
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 [3]                                                          | ["Alan"]                                                     | ["x"]                                                        | [{"a":1}]
(1 row)
```

`like_regex` is true of a string the XQuery regular expression matches
anywhere in it, as LIKE_REGEX in SQL ([Regular
expressions](04-expressions.md#regular-expressions)). Its flags are
letters, in any order:

| Flag | Means |
|---|---|
| `i` | case is ignored |
| `s` | `.` matches a newline too |
| `m` | `^` and `$` match at the ends of each line |
| `x` | white space in the pattern, outside a class, is ignored |
| `q` | the pattern is matched as its characters, none special; `s`, `m` and `x` do nothing beside it |

Another letter is SQLSTATE 2201T, and a pattern that is no regular
expression 2201S, each as the statement is prepared.

```sql
VALUES (JSON_QUERY('["Apple","apricot","Banana"]', 'lax $[*] ? (@ like_regex "^ap" flag "i")' WITH WRAPPER),
        JSON_QUERY('["a.c","abc"]', 'lax $[*] ? (@ like_regex "a.c" flag "q")' WITH WRAPPER));
VALUES JSON_VALUE('"x"', 'lax $ ? (@ like_regex "x" flag "z")');
```

```
                                                              |
--------------------------------------------------------------+--------------------------------------------------------------
 ["Apple","apricot"]                                          | ["a.c"]
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2201T): 'z' is not an XQuery option flag: 'z' is none of s, m, i, x and q [ISO/IEC 9075-2:2023 §9.28 GR5] (at character 25)
```

## Arithmetic

`+`, `-`, `*`, `/` and `%` compute with numbers as SQL does, each
operand one number. An operand of no item or of several is SQLSTATE
22038, and of an item that is no number 22036. A sign applies to each
item its operand gives, each a number, else 2203B. Division by zero is
22012, and a result past the range of its type 22003.

`%` is the remainder of exact whole numbers, as MOD is
[ISO/IEC 9075-2:2023 §6.31 SR12]: of an operand that is approximate or
has a fraction it is SQLSTATE 22000. A whole number written with a
decimal point, `4.0`, is whole.

```sql
VALUES (JSON_VALUE('{"a":7,"b":2}', 'lax $.a % $.b'), JSON_VALUE('{"a":7,"b":2}', 'lax $.a / $.b'),
        JSON_QUERY('[1,2]', 'lax -$[*].double()' WITH ARRAY WRAPPER));
VALUES JSON_VALUE('[1,2]', 'lax $[*] + 1' ERROR ON ERROR);
VALUES JSON_VALUE('5e0', 'lax $ % 2' ERROR ON ERROR);
```

```
                                                              |                                                              |
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 1                                                            | 3.5                                                          | [-1.0E0,-2.0E0]
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22038): an operand of an arithmetic operator is not a singleton numeric value [ISO/IEC 9075-2:2023 §9.46 GR11j ii 1]
turborel: SQLCODE -301 (SQLSTATE 22000): an operand of MOD is not a whole exact or decimal floating-point number of at most 38 digits [ISO/IEC 9075-2:2023 §6.31 SR12]
```

## Item methods

A method applies to each item it is given; in lax mode an array is
first taken for its elements, but by `type()`, `size()` and
`keyvalue()`. The casting methods cast as CAST does, so an item they
take but CAST does not is the cast's exception, as `"x".date()` is
22018.

| Method | Of | Gives | Else |
|---|---|---|---|
| `type()` | any item | its kind: `"null"`, `"boolean"`, `"number"`, `"string"`, `"array"`, `"object"`, `"date"`, `"time without time zone"`, `"time with time zone"`, `"timestamp without time zone"`, `"timestamp with time zone"` | |
| `size()` | an array; any other item, in lax mode | how many elements; 1 of another item | 22039 in strict mode |
| `double()` | a number or a string of one | DOUBLE PRECISION | 22036 |
| `ceiling()`, `floor()`, `abs()` | a number | the least whole number not below it, the greatest not above, its magnitude, of its type; never negative zero | 22036 |
| `bigint()`, `integer()` | a number or a string | BIGINT, INTEGER | 22036 |
| `decimal([p [, s]])` | a number or a string | DECIMAL(p, s), of the defaults if not written | 22036 |
| `number()` | a number or a string | a number of its own type; of a string, of the literal it is | 22036 |
| `boolean()` | a truth value or a string | BOOLEAN | 2202V |
| `string()` | a string, a number, a truth value or a datetime | CHARACTER VARYING(32767) | 2202X |
| `date()` | a string | DATE | 2202W |
| `time([p])`, `timestamp([p])` | a string | TIME(p), TIMESTAMP(p) | 2202Y, 2202Z |
| `time_tz([p])`, `timestamp_tz([p])` | a string; a number, which CAST takes to no datetime, 22005 | TIME(p) WITH TIME ZONE, TIMESTAMP(p) WITH TIME ZONE | 2202Y, 2202Z |
| `datetime()` | a string of a date, time or timestamp literal | the datetime it is | 22031 |
| `datetime("template")` | a string the template reads | the datetime the template makes | 22031 |
| `keyvalue()` | an object | an object for each member: its `key`, `value` and `id` | 2203C |

`datetime()` reads a date, a time or a timestamp, each with a time zone
or without, as its literal is written; `datetime("template")` reads by
the template's fields, as CAST's FORMAT does. It gives a timestamp if
the template has fields of the date and of the time, a date or a time
if of one, with a time zone if it has TZH. A time or timestamp of no time zone that a
comparison or `timestamp_tz()` sets beside one with a zone is taken in
the SQL-session's time zone, with a template and without alike.

```sql
VALUES (JSON_VALUE('[1,2]', 'lax $.size()'), JSON_VALUE('{"a":-1.5}', 'lax $.a.abs()'),
        JSON_VALUE('-0.5e0', 'lax $.ceiling()'), JSON_VALUE('"12.345"', 'lax $.decimal(4,2)'),
        JSON_VALUE('"2024-02-29 10:00:00"', 'lax $.datetime().type()'),
        JSON_VALUE('"29/02/2024"', 'lax $.datetime("DD/MM/YYYY")' RETURNING DATE));
VALUES JSON_VALUE('{}', 'lax $.ceiling()' ERROR ON ERROR);
VALUES JSON_VALUE('"x"', 'lax $.date()' ERROR ON ERROR);
```

```
                                                              |                                                              |                                                              |                                                              |                                                              |
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+------------
 2                                                            | 1.5                                                          | 0E0                                                          | 12.34                                                        | timestamp without time zone                                  | 2024-02-29
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22036): an SQL/JSON item of an item method is not a number [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C IV 1 a]
turborel: SQLCODE -301 (SQLSTATE 22018): 'x' is no DATE literal's string [ISO/IEC 9075:1992 §6.10 GR9a ii, GR10a ii, GR11a ii]
```

## keyvalue()

`keyvalue()` gives an object's members as objects of three members:
`key`, the member's key, `value`, its value, and `id`, a number that
identifies the object the member is of. Each object of the JSON text and
of the PASSING variables has an id of its own, the same however the
path reaches it, so the members of one object share it and those of two
objects differ. Of an item that is no object, keyvalue() is SQLSTATE
2203C, and in lax mode it is applied to each element of an array.

```sql
VALUES JSON_QUERY('{"a":1,"b":{"c":2}}', 'lax $.keyvalue()' RETURNING VARCHAR(100) WITH WRAPPER);
VALUES JSON_QUERY('[{"x":1},{"y":2}]', 'lax $.keyvalue().id' WITH WRAPPER);
VALUES JSON_QUERY('{"price":4,"qty":3}', 'lax $.keyvalue() ? (@.value > 3).key' WITH WRAPPER);
```

```

--------------------------------------------------------------
 [{"key":"a","value":1,"id":0},{"key":"b","value":{"c":2},"id":0}]
(1 row)

--------------------------------------------------------------
 [1,3]
(1 row)

--------------------------------------------------------------
 ["price"]
(1 row)
```

## Variables

`$name` is the value PASSING gives as `name`, an identifier as SQL reads
one. A regular identifier is upper case, so `PASSING 2 AS n` is `$N`. A
string, number, truth value or datetime is the item it is; FORMAT JSON
makes a string's JSON text the item. A variable the path names and
PASSING does not is SQLSTATE 42000, SQLCODE -103, as the statement is
prepared.

```sql
VALUES (JSON_QUERY('[1,5,9]', 'lax $[*] ? (@ > $LOW && @ < $HIGH)' PASSING 2 AS LOW, 9 AS HIGH WITH WRAPPER),
        JSON_VALUE('{}', 'lax $J.a' PASSING '{"a":"x"}' FORMAT JSON AS J),
        JSON_VALUE('{}', 'lax $D.type()' PASSING DATE '2024-02-29' AS D));
```

```
                                                              |                                                              |
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 [5]                                                          | x                                                            | date
(1 row)
```

## Numbers

A number of JSON text is exact if it is written without an exponent,
approximate, DOUBLE PRECISION, if with one. An exact number of more
than 18 significant digits, or a fraction of more than 18 digits, is
DECFLOAT, of at most 34 significant digits; leading zeros and the
trailing zeros of a fraction are not significant. A number of more
significant digits, or one past the range of DOUBLE PRECISION or of
DECFLOAT, is SQLSTATE 22003 wherever JSON text is read, IS JSON too.
A number literal of a path past those limits is refused, SQLCODE -900,
as the statement is prepared.

```sql
VALUES (JSON_VALUE('0.10000000000000000000000000000000000000', 'lax $'),
        JSON_VALUE('1234567890123456789012345678901234', 'lax $.type()'));
VALUES JSON_VALUE('12345678901234567890123456789012345', 'lax $' ERROR ON ERROR);
VALUES '1e400' IS JSON;
```

```
                                                              |
--------------------------------------------------------------+--------------------------------------------------------------
 0.1                                                          | number
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22003): the number has more than 34 significant digits [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A IV]
turborel: SQLCODE -301 (SQLSTATE 22003): the number is too great for an approximate number [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A IV]
```

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 22000 | -301 | `%` of an approximate number or one with a fraction |
| 22003 | -301 | a number past its type's range, or of more than 34 significant digits |
| 22012 | -301 | division by zero, `/` or `%` |
| 2201S, 2201T | -301 | a `like_regex` pattern, or flags, that are none |
| 2202V to 2202Z | -301 | a casting method of an item of a kind it does not take |
| 22031 | -301 | `datetime()` of an item that is no string, or a string it does not read |
| 22033 | -301 | a subscript that is not one number, or in strict mode outside the array |
| 22036 | -301 | a numeric method or operator of an item that is no number |
| 22038 | -301 | an operand of arithmetic that is not one item |
| 22039 | -301 | an array accessor, or in strict mode `size()`, of an item that is no array |
| 2203A | -301 | in strict mode, a member accessor of an item that is no object with the member |
| 2203B | -301 | a sign of an item that is no number |
| 2203C | -301 | in strict mode `.*`, or `keyvalue()`, of an item that is no object |
| 42000 | -103 | a variable PASSING does not give, or a path that is not one |
| | -900 | a path nested deeper than 64, or a number literal past the limits |

Each exception condition of class 22 is one ON ERROR replaces.

## Standard

The language is that of ISO/IEC 9075-2:2023 §9.46, with the item
methods of Features T865 to T878 and `keyvalue()`, `like_regex` and
`starts with`. Hexadecimal number literals are of §5.3. Its
implementation-defined choices are these. `number()` keeps a number's
own type. `string()` is at most 32,767 characters. `keyvalue()`'s id is
each object's place among the items of the text and the variables,
counted in the order they are written from 0, and an object the path
makes is numbered past them.

## See also

[Value Expressions](04-expressions.md#sqljson),
[Predicates](05-predicates.md#json_exists),
[JSON data](../data/03-json.md),
[SQLSTATEs](../appendix/a-sqlstate.md)
