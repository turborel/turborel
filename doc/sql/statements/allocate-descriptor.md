# ALLOCATE DESCRIPTOR

[Statements](README.md)

## NAME

ALLOCATE DESCRIPTOR: allocate an SQL descriptor area

## SYNOPSIS

```
<allocate descriptor statement> ::=
    ALLOCATE DESCRIPTOR <descriptor name> [ WITH MAX <occurrences> ]

<descriptor name> ::= [ GLOBAL | LOCAL ] <simple value specification>

<occurrences> ::= <simple value specification>
```

## DESCRIPTION

An SQL descriptor area describes the dynamic parameters or the columns
of a statement prepared, one item descriptor area for each, and holds
their values: [DESCRIBE](describe.md) fills one,
[GET DESCRIPTOR](get-descriptor.md) reads it,
[SET DESCRIPTOR](set-descriptor.md) sets it, and [EXECUTE](execute.md)
and [FETCH](fetch.md) take values from it and put values in it.

ALLOCATE DESCRIPTOR allocates one, of as many item descriptor areas as
WITH MAX says, 100 if it says none, each undefined, and COUNT 0. Its
name is the value of a literal or a parameter, a character string, its
leading and trailing spaces dropped, which is itself an identifier: of
the session, LOCAL if neither GLOBAL nor LOCAL is written, LOCAL and
GLOBAL names apart. It is kept until
[DEALLOCATE DESCRIPTOR](deallocate-descriptor.md), or the end of the
session, across transactions. A session holds 64 at once.

ALLOCATE DESCRIPTOR runs in direct SQL as well as in a program. It
begins a transaction if none is active.

## RULES

1. The name's value, its spaces dropped, is an identifier, and no area
   of the name and scope is allocated.
2. The occurrences are exact numeric with scale 0, from 1 to 1,000.
3. The session holds fewer than 64 areas.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 33000 | -921 | The name is not an identifier, or an area of it is allocated (rule 1) |
| 2A000 | -103 | The occurrences are not exact with scale 0 (rule 2), or the name not a character string |
| 07009 | -921 | The occurrences are less than 1, or more than 1,000 (rule 2) |
| 0A000 | -900 | The session holds 64 areas (rule 3) |

## EXAMPLES

```sql
ALLOCATE DESCRIPTOR 'STAFF_ROW' WITH MAX 3;
ALLOCATE DESCRIPTOR GLOBAL 'STAFF_ROW';
ALLOCATE DESCRIPTOR 'STAFF_ROW';
ALLOCATE DESCRIPTOR 'TWO WORDS';
ALLOCATE DESCRIPTOR 'BIG' WITH MAX 5000;
COMMIT;
```

```
OK
OK
turborel: SQLCODE -921 (SQLSTATE 33000): SQL descriptor area 'STAFF_ROW' is allocated already [ISO/IEC 9075:1992 §17.2 GR2]
turborel: SQLCODE -921 (SQLSTATE 33000): 'TWO WORDS' is not an <identifier> [ISO/IEC 9075:1992 §17.2 GR1] (at character 20)
turborel: SQLCODE -921 (SQLSTATE 07009): 5000 occurrences: an area has 1 to 1000 [ISO/IEC 9075:1992 §17.2 GR3]
OK
```

## STANDARD

ISO/IEC 9075:1992 §17.2, of the descriptor areas of §17.1 and the names
of §5.4. SQL-92 makes it no `<directly executable statement>` [§20.1]:
direct SQL takes it as a statement of this implementation's. SQL-86 has
no dynamic SQL.

## SEE ALSO

[DEALLOCATE DESCRIPTOR](deallocate-descriptor.md),
[GET DESCRIPTOR](get-descriptor.md), [SET DESCRIPTOR](set-descriptor.md),
[DESCRIBE](describe.md)
