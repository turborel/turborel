# CLOSE

[Statements](README.md)

## NAME

CLOSE: close a cursor *(in a program)*

## SYNOPSIS

```
<close statement> ::= CLOSE <cursor name>
```

## DESCRIPTION

CLOSE ends an open cursor: its rows are let go, and it may be opened
again, its query run anew. The transaction goes on. COMMIT and ROLLBACK
close every open cursor of the transaction.

CLOSE runs only in a program that embeds turborel: see
[DECLARE CURSOR](declare-cursor.md).

## RULES

1. The cursor is open.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | No cursor of the name is declared |
| 24000 | -501 | The cursor is not open |

## EXAMPLES

Of the open cursor C of [FETCH](fetch.md):

```
CLOSE C     Success
CLOSE C     -501 (24000): cursor C is not open [X3.135-1986 §8.1 GR1]
```

## STANDARD

ISO/IEC 9075:1992 §13.4; X3.135-1986 §8.1.

## SEE ALSO

[DECLARE CURSOR](declare-cursor.md), [OPEN](open.md), [FETCH](fetch.md)
