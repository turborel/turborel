# ALLOCATE CURSOR

[Statements](README.md)

## NAME

ALLOCATE CURSOR: allocate a cursor for a query prepared *(in a program)*

## SYNOPSIS

```
<allocate cursor statement> ::=
    ALLOCATE <extended cursor name> [ INSENSITIVE ] [ SCROLL ] CURSOR
    FOR <extended statement name>

<extended cursor name> ::= [ GLOBAL | LOCAL ] <parameter name>
```

The `<extended statement name>` is [PREPARE](prepare.md)'s.

## DESCRIPTION

ALLOCATE CURSOR allocates a cursor for a query prepared under an
extended statement name: [OPEN](open.md) runs the query, with values for
its dynamic parameters, [FETCH](fetch.md) reads its rows, into targets
or an SQL descriptor area, [CLOSE](close.md) closes it; a positioned
[UPDATE](update.md) or [DELETE](delete.md) changes the row it is on. Each
names the cursor by its extended name, as ALLOCATE does. INSENSITIVE,
SCROLL and the query's FOR clause are taken as of a cursor declared
([DECLARE CURSOR](declare-cursor.md)).

The cursor's name is the value of a parameter, a character string, its
leading and trailing spaces dropped, which is itself an identifier: of
the session, LOCAL if neither GLOBAL nor LOCAL is written, LOCAL and
GLOBAL names apart, and apart from the names of cursors declared. It is
kept until the statement is deallocated
([DEALLOCATE PREPARE](deallocate-prepare.md)) or prepared again, or the
session ends; COMMIT and ROLLBACK close it, as any cursor.

A statement prepared may change a row through the cursor by its name:
`DELETE WHERE CURRENT OF RICH`, prepared, deletes the row of the cursor
whose value is RICH; written without a table, it is of the table the
cursor reads.

ALLOCATE CURSOR runs only in a program that embeds turborel, as PREPARE
does. It begins a transaction if none is active.

## RULES

1. A statement of the extended statement name is prepared, and is a
   query.
2. The cursor's name, its spaces dropped, is an identifier, and no cursor
   of it and its scope is allocated.

## PRIVILEGES

As it is opened, those of its query.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 26000 | -921 | No statement of the name is prepared (rule 1) |
| 07005 | -921 | The statement is not a query (rule 1) |
| 34000 | -921 | The cursor's name is not an identifier, or one allocated (rule 2); OPEN, FETCH, CLOSE, UPDATE or DELETE of a cursor not allocated |
| 2A000 | -103 | A name written as a literal |

## EXAMPLES

In a program, of the table STAFF of [DECLARE CURSOR](declare-cursor.md),
the parameters of [PREPARE](prepare.md), FLOOR holding 7000 and CUR the
cursor's name:

```rust
p.set("STMT", Value::Char("BY_PAY".into()));
p.set("TEXT", Value::Char("SELECT ID, NAME FROM STAFF WHERE SALARY > ? FOR UPDATE OF SALARY".into()));
s.execute("PREPARE GLOBAL STMT FROM TEXT", &mut p)?;
p.set("CUR", Value::Char("RICH".into()));
s.execute("ALLOCATE CUR CURSOR FOR GLOBAL STMT", &mut p)?;
s.execute("OPEN CUR USING FLOOR", &mut p)?;
s.execute("FETCH CUR INTO MY_ID, MY_NAME INDICATOR NAME_IND", &mut p)?;    // 1, Ada
s.execute("UPDATE STAFF SET SALARY = SALARY + 100 WHERE CURRENT OF CUR", &mut p)?;
s.execute("FETCH CUR INTO MY_ID, MY_NAME INDICATOR NAME_IND", &mut p)?;    // 2, Grace
```

Refused, RICH open, the first a name allocated, the second of a
statement LOCAL BY_PAY, which is not prepared:

```
ALLOCATE CUR CURSOR FOR GLOBAL STMT       -921 (34000): cursor 'RICH' is allocated already [ISO/IEC 9075:1992 §17.13 GR5]
ALLOCATE GLOBAL CUR CURSOR FOR STMT       -921 (26000): no statement BY_PAY is prepared [ISO/IEC 9075:1992 §17.13 GR1]
OPEN GLOBAL CUR USING FLOOR               -921 (34000): no cursor 'RICH' is allocated [ISO/IEC 9075:1992 §17.14 GR2]
ALLOCATE 'RICH' CURSOR FOR GLOBAL STMT    -103 (2A000): an <extended statement name> or <extended cursor name> is not a <literal> [ISO/IEC 9075:1992 §5.4 SR13]
```

## STANDARD

ISO/IEC 9075:1992 §17.13, of the names of §5.4; the dynamic OPEN, FETCH,
CLOSE and positioned DELETE and UPDATE of §17.14 to §17.18, and the
positioned DELETE and UPDATE prepared of §17.19 and §17.20. SQL-92 makes
none of them a `<directly executable statement>` [§20.1]. SQL-86 has no
dynamic SQL.

## SEE ALSO

[PREPARE](prepare.md), [OPEN](open.md), [FETCH](fetch.md),
[CLOSE](close.md), [DECLARE CURSOR](declare-cursor.md),
[DEALLOCATE PREPARE](deallocate-prepare.md)
