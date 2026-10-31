# DESCRIBE

[Statements](README.md)

## NAME

DESCRIBE: describe a statement prepared in an SQL descriptor area *(in a program)*

## SYNOPSIS

```
<describe statement> ::=
      DESCRIBE INPUT <SQL statement name> <using descriptor>
    | DESCRIBE [ OUTPUT ] <SQL statement name> <using descriptor>

<using descriptor> ::= USING SQL DESCRIPTOR <descriptor name>
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
| 2A000 | -103 | INTO written for USING |
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

The area then holds COUNT 3 and these items:

| Item | TYPE | Other fields | NULLABLE | NAME | UNNAMED |
|---|---|---|---|---|---|
| 1 | 4 (INTEGER) | PRECISION 10, SCALE 0 | 0 | ID | 0 |
| 2 | 12 (CHARACTER VARYING) | LENGTH 30, OCTET_LENGTH 120, SQL_TEXT | 1 | NAME | 0 |
| 3 | 2 (NUMERIC) | PRECISION 18, SCALE 2 | 1 | | 1 |

Then, the row of ID 2 put into it, and its parameter described:

```rust
s.execute("SET DESCRIPTOR 'PAY_ROW' VALUE 2 TYPE = 12, LENGTH = 30", &mut p)?;
p.set("MY_ID", Value::Exact(Decimal::new(2, 0)));
s.execute("EXECUTE PAY INTO SQL DESCRIPTOR 'PAY_ROW' USING MY_ID", &mut p)?;
s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 2 MY_NAME = DATA, N = RETURNED_LENGTH", &mut p)?;  // Grace, 5
s.execute("DESCRIBE INPUT PAY USING SQL DESCRIPTOR 'PAY_ROW'", &mut p)?;
s.execute("GET DESCRIPTOR 'PAY_ROW' VALUE 1 N = TYPE", &mut p)?;                           // 4
```

Refused, and an area SMALL, of 2 occurrences, too small:

```
DESCRIBE GONE USING SQL DESCRIPTOR 'PAY_ROW'   -921 (26000): no statement GONE is prepared [ISO/IEC 9075:1992 §17.8 GR1]
DESCRIBE PAY USING SQL DESCRIPTOR 'NO_ROW'     -921 (33000): no SQL descriptor area 'NO_ROW' is allocated [ISO/IEC 9075:1992 §17.9 GR2]
DESCRIBE PAY INTO SQL DESCRIPTOR 'PAY_ROW'     -103 (2A000): INTO is of a <dynamic fetch statement> or a <result using clause> alone [ISO/IEC 9075:1992 §17.9 SR1]
DESCRIBE PAY USING SQL DESCRIPTOR 'SMALL'      warning 01005: SMALL has 2 occurrences, COUNT 3, no item set
```

## STANDARD

ISO/IEC 9075:1992 §17.8, of the `<using clause>` of §17.9 GR3 and GR4.
SQL-92 makes it no `<directly executable statement>` [§20.1]. SQL-86 has
no dynamic SQL.

## SEE ALSO

[GET DESCRIPTOR](get-descriptor.md), [SET DESCRIPTOR](set-descriptor.md),
[ALLOCATE DESCRIPTOR](allocate-descriptor.md), [PREPARE](prepare.md),
[EXECUTE](execute.md)
