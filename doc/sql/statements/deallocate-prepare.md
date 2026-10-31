# DEALLOCATE PREPARE

[Statements](README.md)

## NAME

DEALLOCATE PREPARE: forget a statement prepared *(in a program)*

## SYNOPSIS

```
<deallocate prepared statement> ::= DEALLOCATE PREPARE <SQL statement name>
```

The `<SQL statement name>` is [PREPARE](prepare.md)'s.

## DESCRIPTION

DEALLOCATE PREPARE forgets a statement prepared, so its name may be
prepared again. With it go the cursors [ALLOCATE CURSOR](allocate-cursor.md)
allocated for it, and the statements prepared that change a row through
one of them, or through a cursor declared for it; a cursor declared for
its name stays declared, without a statement until one of the name is
prepared again. The session's statements are forgotten as it ends.

DEALLOCATE PREPARE runs only in a program that embeds turborel, as
PREPARE does. It begins a transaction if none is active.

## RULES

1. A statement of the name is prepared.
2. No cursor of it is open.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 26000 | -921 | No statement of the name is prepared (rule 1) |
| 24000 | -501 | A cursor of it is open (rule 2) |

## EXAMPLES

In a program, of the statement RAISE_ONE [PREPARE](prepare.md) prepares,
and the cursor RICH of [ALLOCATE CURSOR](allocate-cursor.md), open:

```
DEALLOCATE PREPARE GLOBAL STMT          -501 (24000): statement BY_PAY is of cursor RICH, which is open [ISO/IEC 9075:1992 §17.7 GR2]
CLOSE CUR                               Success
DEALLOCATE PREPARE RAISE_ONE            Success
DEALLOCATE PREPARE GLOBAL STMT          Success: RICH goes with it
EXECUTE RAISE_ONE USING RAISE, MY_ID    -921 (26000): no statement RAISE_ONE is prepared [ISO/IEC 9075:1992 §17.10 GR1]
DEALLOCATE PREPARE RAISE_ONE            -921 (26000): no statement RAISE_ONE is prepared [ISO/IEC 9075:1992 §17.7 GR1]
OPEN CUR USING FLOOR                    -921 (34000): no cursor 'RICH' is allocated [ISO/IEC 9075:1992 §17.14 GR2]
```

## STANDARD

ISO/IEC 9075:1992 §17.7. SQL-92 makes it no `<directly executable
statement>` [§20.1]. SQL-86 has no dynamic SQL.

## SEE ALSO

[PREPARE](prepare.md), [ALLOCATE CURSOR](allocate-cursor.md)
