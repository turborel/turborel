# FETCH

[Statements](README.md)

## NAME

FETCH: move a cursor and read its row *(in a program)*

## SYNOPSIS

```
<fetch statement> ::=
    FETCH [ [ <fetch orientation> ] FROM ] <cursor name>
    INTO <target specification> [ { , <target specification> }... ]

<fetch orientation> ::=
      NEXT | PRIOR | FIRST | LAST
    | { ABSOLUTE | RELATIVE } <simple value specification>

<target specification> ::= <parameter name> [ [ INDICATOR ] <parameter name> ]
```

## DESCRIPTION

FETCH moves an open cursor to a row and assigns the row's values to the
targets, in order: NEXT, the default, to the row after; PRIOR, the row
before; FIRST and LAST; ABSOLUTE n, the nth row, counting from the last
if n is negative; RELATIVE n, n rows on, or back if negative. Past the
last row, or before the first, the cursor is on no row and FETCH
completes with no data, SQLSTATE 02000, the targets unchanged.

A null value sets the target's indicator to -1. A character string
longer than its target is cut to fit, and the indicator holds its
length in characters; FETCH then completes with a warning, SQLSTATE
01004, SQLCODE 0. A bit string is cut so, its length in bits in the
indicator; one shorter than a BIT target is padded with 0 bits, with the
warning 01008. Any other value is assigned to a target as to a column,
by its type.

FETCH runs only in a program that embeds turborel: see
[DECLARE CURSOR](declare-cursor.md).

## RULES

1. The cursor is open.
2. An orientation other than NEXT is of a SCROLL cursor.
3. The targets are as many as the cursor's columns, each a parameter of
   a type its column's value may be assigned to; a null value has a
   target with an indicator.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 24000 | -501 | The cursor is not open |
| 22002 | -301 | A null value, and its target has no indicator |
| 02000 | 100 | Not an error: no data, the cursor past its last row or before its first |
| 01004 | 0 | Not an error: a warning, a string cut to fit its target |
| 01008 | 0 | Not an error: a warning, a bit string padded with 0 bits to fit its target |

## EXAMPLES

Of the table STAFF of [DECLARE CURSOR](declare-cursor.md), its third
row's NAME null, C a cursor of `SELECT ID, NAME FROM STAFF ORDER BY ID`
and S a SCROLL cursor of the same, each open; the targets MY_ID and
MY_NAME, NAME_IND MY_NAME's indicator:

```
FETCH C INTO MY_ID, MY_NAME INDICATOR NAME_IND               1, Ada
FETCH C INTO MY_ID, MY_NAME INDICATOR NAME_IND               2, Grace
FETCH C INTO MY_ID, MY_NAME INDICATOR NAME_IND               3, NAME_IND = -1
FETCH C INTO MY_ID, MY_NAME INDICATOR NAME_IND               no data (02000)
FETCH PRIOR FROM C INTO MY_ID, MY_NAME INDICATOR NAME_IND    -103 (2A000): the cursor is not SCROLL: it fetches NEXT alone [ISO/IEC 9075:1992 §13.3 SR3]
FETCH LAST FROM S INTO MY_ID, MY_NAME INDICATOR NAME_IND     3, NAME_IND = -1
FETCH ABSOLUTE 1 FROM S INTO MY_ID, MY_NAME INDICATOR NAME_IND   1, Ada
FETCH RELATIVE 1 FROM S INTO MY_ID, MY_NAME INDICATOR NAME_IND   2, Grace
FETCH PRIOR FROM S INTO MY_ID, MY_NAME INDICATOR NAME_IND    1, Ada
```

Of a cursor not open:

```
FETCH C INTO MY_ID, MY_NAME INDICATOR NAME_IND   -501 (24000): cursor C is not open [X3.135-1986 §8.6 GR1]
```

Of S, of a target SHORT, CHAR(2), and one with no indicator:

```
FETCH ABSOLUTE -1 FROM S INTO MY_ID, MY_NAME INDICATOR NAME_IND   3, NAME_IND = -1
FETCH ABSOLUTE 1 FROM S INTO MY_ID, SHORT INDICATOR NAME_IND      1, SHORT = 'Ad', NAME_IND = 3; warning 01004
FETCH LAST FROM S INTO MY_ID, MY_NAME                             -301 (22002): the value is null, and target MY_NAME has no indicator parameter [X3.135-1986 §8.6 GR8]
```

## STANDARD

ISO/IEC 9075:1992 §13.3; X3.135-1986 §8.6, whose FETCH moves to the
next row alone.

## SEE ALSO

[DECLARE CURSOR](declare-cursor.md), [OPEN](open.md),
[CLOSE](close.md), [SELECT](select.md)
