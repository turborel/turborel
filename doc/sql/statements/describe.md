# DESCRIBE

[Statements](README.md)

## NAME

DESCRIBE: describe a statement prepared in an SQL descriptor area *(in a program)*

## SYNOPSIS

```
<describe statement> ::=
      DESCRIBE INPUT <SQL statement name> <using descriptor> [ <nesting option> ]
    | DESCRIBE [ OUTPUT ] <SQL statement name> <using descriptor> [ <nesting option> ]

<using descriptor> ::= USING SQL DESCRIPTOR <descriptor name>

<nesting option> ::= WITH NESTING | WITHOUT NESTING
```

The `<SQL statement name>` is [PREPARE](prepare.md)'s, the `<descriptor
name>` [ALLOCATE DESCRIPTOR](allocate-descriptor.md)'s.

## DESCRIPTION

DESCRIBE sets an SQL descriptor area to describe a statement prepared:
DESCRIBE OUTPUT, the default, its columns, of a query, none of another
statement; DESCRIBE INPUT, its dynamic parameters, the `?`s. COUNT is
their number, and the first items each describe one, in order: TYPE and
the fields its type takes, NULLABLE, NAME and UNNAMED, as
[GET DESCRIPTOR](get-descriptor.md) shows. A dynamic parameter's type is
the one it takes from where it stands, as [PREPARE](prepare.md) gives it;
it is nullable, and unnamed.

WITH NESTING, a column or parameter of an array or row type is followed
by items of its parts, in order, each of LEVEL one more than its own. A
row is followed by an item of each field, named as the field is, and an
array by one of its element, of an empty NAME and UNNAMED 1. Each part is
followed in turn by its own parts. COUNT is then the number of every
item, and TOP_LEVEL_COUNT of those of LEVEL 0. WITHOUT NESTING, the
default, sets no item of a part, and COUNT and TOP_LEVEL_COUNT are both
the number of columns or parameters.

DESCRIBE OUTPUT of a query sets KEY_TYPE too, and each column's
KEY_MEMBER. A candidate key of the query's rows is a set of its columns
whose values determine every column's, of which no smaller set does, as
the rules of functional dependency find ([Functionally dependent
columns](../06-queries.md#functionally-dependent-columns)).

KEY_TYPE is 1 if a candidate key is within the selected columns of a
table's PRIMARY KEY, or of grouping columns within one: that key is the
primary key. Otherwise KEY_TYPE is 2, and the key is the preferred
candidate key: one within the columns of a UNIQUE of columns NOT NULL,
or within the grouping columns, else any. Of several, the key is the
left-most: the one whose first column, in the order the query selects
them, comes first, and so on. Each column of the key has KEY_MEMBER 1,
any other column 0, as do a column's parts and a dynamic parameter.
DESCRIBE INPUT sets KEY_TYPE 0.

Of a [CALL](call.md), DESCRIBE INPUT describes its input `?`s, those of
its IN and INOUT parameters' arguments, and DESCRIBE OUTPUT its output
ones, those of its OUT and INOUT parameters' arguments, in the order of
the parameters. Each item names the parameter whose argument holds its
`?`. PARAMETER_MODE is that parameter's mode, 1 of IN, 2 of INOUT and 4
of OUT, and PARAMETER_ORDINAL_POSITION its position, from 1. The
PARAMETER_SPECIFIC_CATALOG, PARAMETER_SPECIFIC_SCHEMA and
PARAMETER_SPECIFIC_NAME fields name the procedure, by its specific name.
Several `?` marks of one IN argument each have that parameter's fields.
Of another statement, and of a part of an item, they are 0 and empty.

A candidate key may not be unique. A query that selects rows alike
still has one, all its columns. A query of at most one row has the empty
set as its key, of no member.

If they are more than the area's occurrences, COUNT is their number,
no item is set, and DESCRIBE completes with a warning, SQLSTATE 01005,
SQLCODE 0: the area is too small.

An area DESCRIBE OUTPUT set is ready for [FETCH](fetch.md) and
[EXECUTE](execute.md) to put a row's values in; one DESCRIBE INPUT set,
once its DATA is set ([SET DESCRIPTOR](set-descriptor.md)), to give a
statement's parameters their values.

DESCRIBE runs only in a program that embeds turborel, as PREPARE does.
It begins a transaction if none is active.

## RULES

1. A statement of the name is prepared.
2. An area of the descriptor's name is allocated.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 26000 | -921 | No statement of the name is prepared (rule 1) |
| 33000 | -921 | No area of the name is allocated (rule 2) |
| 42000 | -103 | INTO written for USING |
| 01005 | 0 | Not an error: a warning, more columns or parameters than the area's occurrences |

## EXAMPLES

In a program, of the table STAFF of [DECLARE CURSOR](declare-cursor.md)
and the parameters of [PREPARE](prepare.md) and
[GET DESCRIPTOR](get-descriptor.md), TEXT holding `SELECT ID, NAME,
SALARY * 2 FROM STAFF WHERE ID = ?`:

```rust
s.execute("PREPARE PAY FROM TEXT", &mut p)?;
s.execute("ALLOCATE DESCRIPTOR 'PAY_ROW' WITH MAX 3", &mut p)?;
s.execute("DESCRIBE OUTPUT PAY USING SQL DESCRIPTOR 'PAY_ROW'", &mut p)?;
```

The area then holds COUNT 3, KEY_TYPE 1, and these items. `ID = ?`
makes ID one value, so the query has at most one row, and its primary
key is the empty set, within the table's:

| Item | TYPE | Other fields | NULLABLE | NAME | UNNAMED | KEY_MEMBER |
|---|---|---|---|---|---|---|
| 1 | 4 (INTEGER) | PRECISION 10, SCALE 0 | 0 | ID | 0 | 0 |
| 2 | 12 (CHARACTER VARYING) | LENGTH 30, OCTET_LENGTH 120, SQL_TEXT | 1 | NAME | 0 | 0 |
| 3 | 2 (NUMERIC) | PRECISION 10, SCALE 2 | 1 | | 1 | 0 |

Of `SELECT NAME, ID FROM STAFF`, KEY_TYPE is 1, and ID's KEY_MEMBER 1.

Then, the row of ID 2 put into it, and its parameter described:

```rust
s.execute("SET DESCRIPTOR 'PAY_ROW' VALUE 2 TYPE = 12, LENGTH = 30", &mut p)?;
p.set("MY_ID", Value::Exact(Decimal::new(2, 0)));
s.execute("EXECUTE PAY INTO SQL DESCRIPTOR 'PAY_ROW' USING MY_ID", &mut p)?;
s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 2 MY_NAME = DATA, N = RETURNED_LENGTH", &mut p)?;  // Grace, 5
s.execute("DESCRIBE INPUT PAY USING SQL DESCRIPTOR 'PAY_ROW'", &mut p)?;
s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 1 N = TYPE", &mut p)?;                           // 4
```

Of the procedure `MIXED (OUT TOTAL INTEGER, IN TIMES INTEGER, INOUT TAG
VARCHAR(10))`, created first and so of the specific name MIXED_1
([CREATE PROCEDURE](create-procedure.md)), TEXT holding `CALL MIXED(?, ?, ?)`:

```rust
s.execute("PREPARE MIX FROM TEXT", &mut p)?;
s.execute("DESCRIBE INPUT MIX USING SQL DESCRIPTOR 'PAY_ROW'", &mut p)?;
s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 2 N = PARAMETER_MODE, MY_ID = PARAMETER_ORDINAL_POSITION", &mut p)?;  // 2, 3
s.execute("DESCRIBE OUTPUT MIX USING SQL DESCRIPTOR 'PAY_ROW'", &mut p)?;
s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 1 N = PARAMETER_MODE, MY_NAME = PARAMETER_SPECIFIC_NAME", &mut p)?;  // 4, MIXED_1
```

The input items are TIMES, IN of position 2, and TAG, INOUT of 3. The
output items are TOTAL, OUT of 1, and TAG.

Refused, and an area SMALL, of 2 occurrences, too small:

```
DESCRIBE GONE USING SQL DESCRIPTOR 'PAY_ROW'   -921 (26000): no statement GONE is prepared [ISO/IEC 9075:1992 §17.8 GR1]
DESCRIBE PAY USING SQL DESCRIPTOR 'NO_ROW'     -921 (33000): no SQL descriptor area 'NO_ROW' is allocated [ISO/IEC 9075:1992 §17.9 GR2]
DESCRIBE PAY INTO SQL DESCRIPTOR 'PAY_ROW'     -103 (42000): INTO is of a <dynamic fetch statement> or a <result using clause> alone [ISO/IEC 9075:1992 §17.9 SR1]
DESCRIBE PAY USING SQL DESCRIPTOR 'SMALL'      warning 01005: SMALL has 2 occurrences, COUNT 3, no item set
```

## STANDARD

ISO/IEC 9075:1992 §17.8, of the `<using clause>` of §17.9 GR3 and GR4.
The `<nesting option>` is ISO/IEC 9075-2:2023's [§20.9, §20.10 GR6f,
GR7], and so are KEY_TYPE and KEY_MEMBER [§20.10 GR6d, GR7c, §4.27],
Feature T301, and the PARAMETER fields of a CALL's items [§20.10 GR7e,
Table 33].
SQL-92 makes it no `<directly executable statement>` [§20.1]. SQL-86 has
no dynamic SQL.

## SEE ALSO

[GET DESCRIPTOR](get-descriptor.md), [SET DESCRIPTOR](set-descriptor.md),
[ALLOCATE DESCRIPTOR](allocate-descriptor.md), [PREPARE](prepare.md),
[EXECUTE](execute.md)
