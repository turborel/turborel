# GET DESCRIPTOR

[Statements](README.md)

## NAME

GET DESCRIPTOR: read fields of an SQL descriptor area *(in a program)*

## SYNOPSIS

```
<get descriptor statement> ::=
    GET DESCRIPTOR <descriptor name>
    { <parameter name> = COUNT
    | VALUE <item number> <get item information> [ { , <get item information> }... ] }

<get item information> ::= <parameter name> = <descriptor item name>

<item number> ::= <simple value specification>

<descriptor item name> ::=
      TYPE | LENGTH | OCTET_LENGTH | RETURNED_LENGTH | RETURNED_OCTET_LENGTH
    | PRECISION | SCALE | DATETIME_INTERVAL_CODE | DATETIME_INTERVAL_PRECISION
    | NULLABLE | INDICATOR | DATA | NAME | UNNAMED
    | COLLATION_CATALOG | COLLATION_SCHEMA | COLLATION_NAME
    | CHARACTER_SET_CATALOG | CHARACTER_SET_SCHEMA | CHARACTER_SET_NAME
```

The `<descriptor name>` is [ALLOCATE DESCRIPTOR](allocate-descriptor.md)'s.

## DESCRIPTION

GET DESCRIPTOR copies COUNT, or fields of the item descriptor area of
the number given, from 1, into parameters. An item beyond COUNT is no
data, SQLSTATE 02000, its targets unchanged. A field never set reads as
0, or the empty string, and DATA as the null value.

The fields of an item, of a dynamic parameter or a column, as
[DESCRIBE](describe.md) sets them:

| Field | Type | Value |
|---|---|---|
| TYPE | exact | The data type's code, below |
| LENGTH | exact | A string's length, in characters or bits; a datetime's length in positions |
| OCTET_LENGTH | exact | A string's length in octets at most: 4 to a character |
| RETURNED_LENGTH, RETURNED_OCTET_LENGTH | exact | Of a varying string fetched, its length, and in octets |
| PRECISION | exact | A number's precision; a datetime's or interval's fractional seconds precision |
| SCALE | exact | An exact number's scale |
| DATETIME_INTERVAL_CODE | exact | A datetime's or interval's code, below |
| DATETIME_INTERVAL_PRECISION | exact | An interval's leading field precision |
| NULLABLE | exact | 1 if it may be null, else 0; 1 of a dynamic parameter |
| INDICATOR | exact | -1 of a null value fetched, else 0 |
| DATA | the item's type | The value |
| NAME | character | A column's name; empty of an unnamed column and of a dynamic parameter |
| UNNAMED | exact | 1 of an unnamed column and of a dynamic parameter, else 0 |
| CHARACTER_SET_CATALOG, _SCHEMA, _NAME | character | A string's character set: the catalog, INFORMATION_SCHEMA, SQL_TEXT |
| COLLATION_CATALOG, _SCHEMA, _NAME | character | A string's collation, named as its character set |

The codes of TYPE:

| Code | Type | Code | Type |
|---|---|---|---|
| 1 | CHARACTER | 8 | DOUBLE PRECISION |
| 2 | NUMERIC | 9 | DATE, TIME or TIMESTAMP |
| 3 | DECIMAL | 10 | INTERVAL |
| 4 | INTEGER | 12 | CHARACTER VARYING |
| 5 | SMALLINT | 14 | BIT |
| 6 | FLOAT | 15 | BIT VARYING |
| 7 | REAL | -1 | a type of a plug-in |

The codes of DATETIME_INTERVAL_CODE, of a datetime: 1 DATE, 2 TIME, 3
TIMESTAMP, 4 TIME WITH TIME ZONE, 5 TIMESTAMP WITH TIME ZONE; of an
interval: 1 YEAR, 2 MONTH, 3 DAY, 4 HOUR, 5 MINUTE, 6 SECOND, 7 YEAR TO
MONTH, 8 DAY TO HOUR, 9 DAY TO MINUTE, 10 DAY TO SECOND, 11 HOUR TO
MINUTE, 12 HOUR TO SECOND, 13 MINUTE TO SECOND.

GET DESCRIPTOR runs only in a program that embeds turborel, through
`Session::execute`: its targets are parameters, which direct SQL has
none of.

## RULES

1. An area of the name is allocated.
2. The item number is exact numeric with scale 0, from 1 to the area's
   occurrences.
3. COUNT's target and a numeric field's are exact numeric with scale 0;
   a name's field's a character string of 128 characters or more; DATA's
   of the item's type, a varying string's of its length or longer.
4. DATA of a negative INDICATOR is got with INDICATOR.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 33000 | -921 | No area of the name is allocated (rule 1) |
| 2A000 | -103 | The item number or a target not of its type (rules 2, 3) |
| 07009 | -921 | The item number is not from 1 to the occurrences (rule 2) |
| 22005 | -301 | DATA's target is not of the item's type (rule 3) |
| 22002 | -301 | DATA of a negative INDICATOR, without INDICATOR (rule 4); with it, DATA's target is left as it is |
| 02000 | 100 | Not an error: no data, the item is beyond COUNT |

## EXAMPLES

In a program, of the parameters [PREPARE](prepare.md) declares, and N,
exact, and WHAT, CHARACTER(128); MY_NAME holding `Ada`:

```rust
s.execute("ALLOCATE DESCRIPTOR 'ROW' WITH MAX 3", &mut p)?;
s.execute("SET DESCRIPTOR 'ROW' COUNT = 1", &mut p)?;
s.execute("SET DESCRIPTOR 'ROW' VALUE 1 TYPE = 12, LENGTH = 30, DATA = MY_NAME", &mut p)?;
s.execute("GET DESCRIPTOR 'ROW' N = COUNT", &mut p)?;                                   // N = 1
s.execute("GET DESCRIPTOR 'ROW' VALUE 1 N = TYPE, WHAT = CHARACTER_SET_NAME", &mut p)?;  // 12, SQL_TEXT
s.execute("GET DESCRIPTOR 'ROW' VALUE 1 TEXT = DATA", &mut p)?;                         // TEXT = Ada
s.execute("GET DESCRIPTOR 'ROW' VALUE 2 N = TYPE", &mut p)?;                            // no data
s.execute("SET DESCRIPTOR 'ROW' VALUE 1 INDICATOR = -1", &mut p)?;
```

Refused, then:

```
GET DESCRIPTOR 'ROW' VALUE 1 TEXT = DATA    -301 (22002): INDICATOR is negative, and DATA is got without it [ISO/IEC 9075:1992 §17.4 GR5]
GET DESCRIPTOR 'ROW' VALUE 1 MY_ID = DATA   -301 (22005): NUMERIC(9,0) does not match the type the item describes [ISO/IEC 9075:1992 §17.4 GR2, §17.1 SR2]
GET DESCRIPTOR 'ROW' VALUE 4 N = TYPE       -921 (07009): item 4 is not of 1 to 3, the area's occurrences [ISO/IEC 9075:1992 §17.4 GR3]
GET DESCRIPTOR 'ROW' VALUE 1 WHAT = TYPE    -103 (2A000): the target of TYPE is not of its type [ISO/IEC 9075:1992 §17.4 SR3, Table 17]
GET DESCRIPTOR 'NO_ROW' N = COUNT           -921 (33000): no SQL descriptor area 'NO_ROW' is allocated [ISO/IEC 9075:1992 §17.4 GR1]
```

## STANDARD

ISO/IEC 9075:1992 §17.4, of the descriptor areas of §17.1, Tables 17 to
20. SQL-92 makes it no `<directly executable statement>` [§20.1]. SQL-86
has no dynamic SQL.

## SEE ALSO

[SET DESCRIPTOR](set-descriptor.md), [DESCRIBE](describe.md),
[ALLOCATE DESCRIPTOR](allocate-descriptor.md),
[GET DIAGNOSTICS](get-diagnostics.md)
