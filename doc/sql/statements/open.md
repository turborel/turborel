# OPEN

[Statements](README.md)

## NAME

OPEN: open a cursor *(in a program)*

## SYNOPSIS

```
<open statement> ::= OPEN <cursor name>
```

## DESCRIPTION

OPEN runs a cursor's query, with the values its parameters have now,
and puts the cursor before its first row. It begins a transaction if
none is active. The cursor stays open until [CLOSE](close.md), or until
the transaction ends: COMMIT and ROLLBACK close it.

OPEN runs only in a program that embeds turborel: see
[DECLARE CURSOR](declare-cursor.md).

## RULES

1. The cursor is declared, and not open.
2. Its specification follows the rules of
   [DECLARE CURSOR](declare-cursor.md); each parameter it names has a
   value, or an indicator that says null.

## PRIVILEGES

SELECT on the tables the query reads.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | No cursor of the name is declared, or a rule of its specification is broken |
| 2A000 | -104 | A privilege the query needs is not held |
| 24000 | -501 | The cursor is open |

## EXAMPLES

Of the cursor C, `DECLARE C CURSOR FOR SELECT ID, NAME FROM STAFF ORDER
BY ID`:

```
OPEN C          Success
OPEN C          -501 (24000): cursor C is already open [X3.135-1986 §8.8 GR1]
OPEN NOPE       -103 (2A000): no cursor NOPE is declared [X3.135-1986 §8.8 SR1]
```

## STANDARD

ISO/IEC 9075:1992 §13.2; X3.135-1986 §8.8.

## SEE ALSO

[DECLARE CURSOR](declare-cursor.md), [FETCH](fetch.md),
[CLOSE](close.md)
