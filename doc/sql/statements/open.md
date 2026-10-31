# OPEN

[Statements](README.md)

## NAME

OPEN: open a cursor *(in a program)*

## SYNOPSIS

```
<open statement> ::= OPEN <cursor name>

<dynamic open statement> ::=
    OPEN <dynamic cursor name>
    [ USING <target specification> [ { , <target specification> }... ]
    | USING SQL DESCRIPTOR <descriptor name> ]

<dynamic cursor name> ::= <cursor name> | <extended cursor name>
```

The `<extended cursor name>` is [ALLOCATE CURSOR](allocate-cursor.md)'s,
the `<descriptor name>` [ALLOCATE DESCRIPTOR](allocate-descriptor.md)'s.

## DESCRIPTION

OPEN runs a cursor's query, with the values its parameters have now,
and puts the cursor before its first row. It begins a transaction if
none is active. The cursor stays open until [CLOSE](close.md), or until
the transaction ends: COMMIT and ROLLBACK close it.

A dynamic cursor, declared for a statement name or allocated, is opened
of the query prepared for it, its dynamic parameters given their values
by the USING clause, as [EXECUTE](execute.md) gives them; a query of
none needs no USING clause. The diagnostics area names a dynamic OPEN
`DYNAMIC OPEN`.

OPEN runs only in a program that embeds turborel: see
[DECLARE CURSOR](declare-cursor.md).

## RULES

1. The cursor is declared, and not open.
2. Its specification follows the rules of
   [DECLARE CURSOR](declare-cursor.md); each parameter it names has a
   value, or an indicator that says null.
3. A USING clause is of a dynamic cursor, whose query is prepared; it
   gives a value for each dynamic parameter, as EXECUTE's does.

## PRIVILEGES

SELECT on the tables the query reads.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | No cursor of the name is declared, or a rule of its specification is broken |
| 2A000 | -104 | A privilege the query needs is not held |
| 24000 | -501 | The cursor is open |
| 26000 | -921 | A dynamic cursor's statement is not prepared (rule 3) |
| 34000 | -921 | No cursor of the extended name is allocated |
| 07004 | -921 | A dynamic cursor's query has dynamic parameters, and no USING clause (rule 3) |
| 07001, 07006, 07008 | -921 | The USING clause does not give a valid value for each parameter, as of [EXECUTE](execute.md) (rule 3) |

## EXAMPLES

Of the cursor C, `DECLARE C CURSOR FOR SELECT ID, NAME FROM STAFF ORDER
BY ID`:

```
OPEN C          Success
OPEN C          -501 (24000): cursor C is already open [X3.135-1986 §8.8 GR1]
OPEN NOPE       -103 (2A000): no cursor NOPE is declared [X3.135-1986 §8.8 SR1]
```

Of the dynamic cursor D of [DECLARE CURSOR](declare-cursor.md):

```
OPEN D USING FLOOR    -921 (26000): no statement NAME_LIST is prepared [ISO/IEC 9075:1992 §17.14 GR1]
OPEN D                -921 (07004): the statement has dynamic parameters, and no <using clause> [ISO/IEC 9075:1992 §17.14 GR3]
OPEN D USING FLOOR    Success
OPEN D USING FLOOR    -501 (24000): cursor D is already open [X3.135-1986 §8.8 GR1]
```

## STANDARD

ISO/IEC 9075:1992 §13.2, and of a dynamic cursor §17.14; X3.135-1986
§8.8.

## SEE ALSO

[DECLARE CURSOR](declare-cursor.md), [FETCH](fetch.md),
[CLOSE](close.md), [ALLOCATE CURSOR](allocate-cursor.md)
