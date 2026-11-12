# ALTER SEQUENCE

[Statements](README.md)

## NAME

ALTER SEQUENCE: change a sequence generator

## SYNOPSIS

```
<alter sequence generator statement> ::=
    ALTER SEQUENCE <sequence generator name> <alter sequence generator option>...

<alter sequence generator option> ::=
      RESTART [ WITH <signed numeric literal> ]
    | INCREMENT BY <signed numeric literal>
    | MAXVALUE <signed numeric literal> | NO MAXVALUE
    | MINVALUE <signed numeric literal> | NO MINVALUE
    | CYCLE | NO CYCLE
```

## DESCRIPTION

ALTER SEQUENCE changes the options of a generator CREATE SEQUENCE made. An
option not written keeps its value. NO MAXVALUE and NO MINVALUE give the
defaults again, as of the increment the generator has after the change.

RESTART makes the generator go on from its start, and RESTART WITH from the
value given. Without RESTART it goes on from where it was, a new increment
counting from there.

The change runs within the session's transaction. A value taken meanwhile,
by this session or another, is never given again, as with any value a
generator gives.

## RULES

1. The generator exists.
2. Each option is written at most once.
3. Each value is a whole number the generator's type holds.
4. The increment is not 0, and the maximum is greater than the minimum.
5. A restart value is within the bounds. Without RESTART, the next value is
   within them, unless the generator has given its last value.

## PRIVILEGES

The owner of the generator's schema.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the generator's schema |

## EXAMPLES

```sql
CREATE SEQUENCE TICKET START WITH 10;
SELECT NEXT VALUE FOR TICKET FROM (VALUES (0)) AS X (Z);
ALTER SEQUENCE TICKET INCREMENT BY 5;
SELECT NEXT VALUE FOR TICKET FROM (VALUES (0)) AS X (Z);
ALTER SEQUENCE TICKET RESTART;
SELECT NEXT VALUE FOR TICKET FROM (VALUES (0)) AS X (Z);
ALTER SEQUENCE TICKET MAXVALUE 5;
```

```
OK

----------------------
                   10
(1 row)
OK

----------------------
                   11
(1 row)
OK

----------------------
                   10
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): a sequence generator's current base value is within its bounds, and 15 is not within 1 to 5 [ISO/IEC 9075-2:2023 §9.37 SR12] (at character 15)
```

## STANDARD

ISO/IEC 9075-2:2023 §9.37, §11.73, Features T176 and T177. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sequence-generators).

## SEE ALSO

[CREATE SEQUENCE](create-sequence.md), [DROP SEQUENCE](drop-sequence.md)
