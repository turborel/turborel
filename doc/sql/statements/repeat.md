# REPEAT

[Statements](README.md)

## NAME

REPEAT: run statements until a condition is true

## SYNOPSIS

```
<repeat statement> ::=
    [ <beginning label> : ] REPEAT
    <statement list>
    UNTIL <search condition>
    END REPEAT [ <ending label> ]

<statement list> ::= { <SQL procedure statement> ; }...
```

END is a key word. REPEAT and UNTIL are read as words, not key words. A
label is an identifier.

## DESCRIPTION

REPEAT runs its statements, then reads its condition. If it is true,
REPEAT ends. If it is false or unknown, REPEAT begins again. So the
statements run at least once.

[LEAVE](leave.md) of its label ends it at once. [ITERATE](iterate.md)
of its label skips the rest of the statements, and the condition is
read as at the end of an iteration.

REPEAT stands in a [compound statement](begin.md), a routine's body or
a trigger's action, and may be run directly too.

A loop whose condition never becomes true is cancelled as any statement
is, with SQLCODE -920, and the session goes on.

## RULES

1. An ending label is the loop's beginning label.
2. A beginning label is not the label of a statement the loop is in.
3. The list holds one statement or more, each ended by a semicolon.
4. REPEAT is not preparable.

## PRIVILEGES

Those of its statements and condition.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | It is not well formed, as a REPEAT with no UNTIL |
| 2A000 | -103 | A rule above is broken |
| TB920 | -920 | It is cancelled |

## EXAMPLES

A countdown, and a REPEAT whose condition is true at once still runs
once:

```sql
CREATE TABLE COUNTDOWN (N INTEGER);
BEGIN
  DECLARE I INTEGER DEFAULT 3;
  REPEAT
    INSERT INTO COUNTDOWN VALUES (I);
    SET I = I - 1;
  UNTIL I = 0 END REPEAT;
  REPEAT
    INSERT INTO COUNTDOWN VALUES (100);
  UNTIL TRUE END REPEAT;
END;
SELECT N FROM COUNTDOWN ORDER BY N;
```

```
OK
OK
            N
--------------
            1
            2
            3
          100
(4 rows)
```

ITERATE reads the condition too, and a REPEAT without UNTIL:

```sql
CREATE TABLE EVENS (N INTEGER);
BEGIN
  DECLARE I INTEGER DEFAULT 0;
  R: REPEAT
    SET I = I + 1;
    IF I IN (1, 3, 5) THEN ITERATE R; END IF;
    INSERT INTO EVENS VALUES (I);
  UNTIL I >= 5 END REPEAT R;
END;
SELECT N FROM EVENS ORDER BY N;
BEGIN DECLARE I INTEGER DEFAULT 0; REPEAT SET I = I + 1; END REPEAT; END;
```

```
OK
OK
            N
--------------
            2
            4
(2 rows)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected UNTIL, found END (at character 57)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.12, Feature P002. SQL-86 and SQL-92 have
no such statement.

## SEE ALSO

[WHILE](while.md), [LOOP](loop.md), [FOR](for.md), [LEAVE](leave.md),
[ITERATE](iterate.md), [BEGIN](begin.md)
