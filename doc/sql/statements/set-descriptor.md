# SET DESCRIPTOR

[Statements](README.md)

## NAME

SET DESCRIPTOR: set fields of an SQL descriptor area

## SYNOPSIS

```
<set descriptor statement> ::=
    SET DESCRIPTOR <descriptor name>
    { COUNT = <simple value specification>
    | VALUE <item number> <set item information> [ { , <set item information> }... ] }

<set item information> ::= <descriptor item name> = <simple value specification>

<item number> ::= <simple value specification>
```

The `<descriptor name>` is [ALLOCATE DESCRIPTOR](allocate-descriptor.md)'s,
the `<descriptor item name>`s [GET DESCRIPTOR](get-descriptor.md)'s.

## DESCRIPTION

SET DESCRIPTOR sets COUNT, the number of items in use, or fields of the
item descriptor area of the number given, from 1: as [EXECUTE](execute.md)
or [OPEN](open.md) will take a dynamic parameter's value from it, or
[FETCH](fetch.md) put a column's value in it.

The fields are set in the order LEVEL, TYPE, DATETIME_INTERVAL_CODE,
DATETIME_INTERVAL_PRECISION, PRECISION, SCALE, CHARACTER_SET_CATALOG,
CHARACTER_SET_SCHEMA, CHARACTER_SET_NAME, LENGTH, INDICATOR, DEGREE,
CARDINALITY and DATA, whatever the order written. Setting a field other
than DATA makes DATA undefined. Setting TYPE makes the other fields
undefined but LEVEL and these:

| TYPE | Set with it |
|---|---|
| CHARACTER, CHARACTER VARYING | LENGTH 1; the character set SQL_TEXT, of INFORMATION_SCHEMA |
| BINARY, BINARY VARYING | LENGTH 1 |
| BOOLEAN | nothing |
| NUMERIC, DECIMAL | PRECISION 18, SCALE 0 |
| FLOAT | PRECISION 53 |
| DECFLOAT | PRECISION 34 |
| a datetime | PRECISION 0 |
| INTERVAL | DATETIME_INTERVAL_PRECISION 2 |

Setting DATETIME_INTERVAL_CODE of a datetime sets PRECISION 6 of a
TIMESTAMP, else 0; of an interval, DATETIME_INTERVAL_PRECISION 2, and
PRECISION 6 if it ends in SECOND, else 0.

An item of TYPE 19, ROW, of DEGREE n is followed by the items of its n
fields, of LEVEL one more than its own; one of TYPE 50, ARRAY, of
CARDINALITY its maximum cardinality, by the item of its element. Each such
item is followed in turn by those of its own parts, as
[DESCRIBE](describe.md) WITH NESTING sets them. The first item is of
LEVEL 0. Another's LEVEL is one more than the item before it if that one
is of TYPE ROW or ARRAY; else no more than that one's, closing each row
whose fields it follows with all of them, and if more than 0, a field of
a row not yet given all its fields. A field's NAME, which DESCRIBE sets, has
no part in the type the items describe.

DATA is of the type the item describes, exactly: a NUMERIC or DECIMAL of
its PRECISION and SCALE, an INTEGER, SMALLINT, BIGINT, REAL or DOUBLE PRECISION
of that type, a FLOAT or a DECFLOAT of its PRECISION; a string of its LENGTH, a
CHARACTER of CHARACTER, a CHARACTER VARYING of CHARACTER VARYING; a
datetime or interval of its type.

SET DESCRIPTOR runs in direct SQL as well as in a program, its values
literals or parameters. It begins a transaction if none is active.

## RULES

1. An area of the name is allocated.
2. COUNT and the item number are exact numeric with scale 0; the item
   number is from 1 to the area's occurrences.
3. Each field is set to a value of its type: exact numeric with scale 0,
   or of a character set's name a character string; DATA to a value of
   the item's type.
4. LEVEL is set in the order above.
5. Neither TOP_LEVEL_COUNT, NAME, UNNAMED, NULLABLE, OCTET_LENGTH, RETURNED_LENGTH,
   RETURNED_OCTET_LENGTH, RETURNED_CARDINALITY nor the collation's
   fields is set; no field twice.
6. DATA is not set of an item of TYPE ROW, nor of an item that is part of
   an array's element.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 33000 | -921 | No area of the name is allocated (rule 1) |
| 2A000 | -103 | A value not of its field's type (rules 2, 3), or a field not set or set twice (rule 5) |
| 0700E | -921 | LEVEL out of the order the items take (rule 4) |
| 0700D | -921 | DATA of a row or of an array's element (rule 6) |
| 07009 | -921 | The item number is not from 1 to the occurrences (rule 2) |
| 22005 | -301 | DATA's type is not the item's (rule 3) |

## EXAMPLES

Of the area STAFF_ROW of 3 occurrences
[ALLOCATE DESCRIPTOR](allocate-descriptor.md) allocates: values for a
statement of two dynamic parameters, the first NUMERIC(1,0), the second
CHARACTER(3), of literals:

```sql
SET DESCRIPTOR 'STAFF_ROW' COUNT = 2;
SET DESCRIPTOR 'STAFF_ROW' VALUE 1 TYPE = 2, PRECISION = 1, DATA = 4;
SET DESCRIPTOR 'STAFF_ROW' VALUE 2 DATA = 'Eve', TYPE = 1, LENGTH = 3;
```

```
OK
OK
OK
```

Refused:

```sql
SET DESCRIPTOR 'STAFF_ROW' VALUE 2 DATA = 'Eva Lu';
SET DESCRIPTOR 'STAFF_ROW' VALUE 4 TYPE = 4;
SET DESCRIPTOR 'STAFF_ROW' VALUE 1 NAME = 'ID';
SET DESCRIPTOR 'STAFF' COUNT = 1;
```

```
turborel: SQLCODE -301 (SQLSTATE 22005): CHARACTER(6) does not match the type the item describes [ISO/IEC 9075:1992 §17.5 GR5, §17.1 SR2]
turborel: SQLCODE -921 (SQLSTATE 07009): item 4 is not of 1 to 3, the area's occurrences [ISO/IEC 9075:1992 §17.5 GR2]
turborel: SQLCODE -103 (SQLSTATE 2A000): NAME is not set here, or is set twice [ISO/IEC 9075:1992 §17.5 SR3] (at character 35)
turborel: SQLCODE -921 (SQLSTATE 33000): no SQL descriptor area 'STAFF' is allocated [ISO/IEC 9075:1992 §17.5 GR1]
```

## STANDARD

ISO/IEC 9075:1992 §17.5, of the descriptor areas of §17.1. SQL-92 makes
it no `<directly executable statement>` [§20.1]: direct SQL takes it as
a statement of this implementation's. SQL-86 has no dynamic SQL. TYPE
set to 60, BINARY, sets LENGTH 1 [ISO/IEC 9075-2:2023 §20.5 GR4c iii 3],
and set to 26, DECFLOAT, PRECISION 34 [GR4c iii 8]. LEVEL, DEGREE,
CARDINALITY and the codes 19 and 50 are of §20.1 SR3 to SR6 and §20.5
GR4b of it.

## SEE ALSO

[GET DESCRIPTOR](get-descriptor.md), [ALLOCATE DESCRIPTOR](allocate-descriptor.md),
[EXECUTE](execute.md), [DESCRIBE](describe.md)
