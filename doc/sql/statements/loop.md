# LOOP

[Statements](README.md)

## NAME

LOOP: run statements again and again until something leaves

## SYNOPSIS

```
<loop statement> ::=
    [ <beginning label> : ] LOOP <statement list> END LOOP [ <ending label> ]

<statement list> ::= { <SQL procedure statement> ; }...
```

END is a key word. LOOP is read as a word, not a key word. A label is
an identifier.

## DESCRIPTION

LOOP runs its statements, then runs them again, without end. It ends
when [LEAVE](leave.md) names it or a statement it is in, when an
exception no handler handles ends it, or when a handler's EXIT leaves
the compound statement it is in. [ITERATE](iterate.md) of its label
begins its next iteration at once.

LOOP stands in a [compound statement](begin.md), a routine's body or a
trigger's action, and may be run directly too.

A loop that never ends is cancelled as any statement is, with SQLCODE
-920, and the session goes on.

## RULES

1. An ending label is the loop's beginning label.
2. A beginning label is not the label of a statement the loop is in.
3. The list holds one statement or more, each ended by a semicolon.
4. LOOP is not preparable.

## PRIVILEGES

Those of its statements.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | It is not well formed, as a LOOP ended by END WHILE |
| 42000 | -103 | A rule above is broken |
| TB920 | -920 | It is cancelled |

## EXAMPLES

Doubling until past 100:

```sql
CREATE TABLE POWERS (N INTEGER);
BEGIN
  DECLARE P INTEGER DEFAULT 1;
  GROWING: LOOP
    INSERT INTO POWERS VALUES (P);
    SET P = P * 2;
    IF P > 100 THEN
      LEAVE GROWING;
    END IF;
  END LOOP GROWING;
END;
SELECT N FROM POWERS ORDER BY N;
```

```
OK
OK
            N
--------------
            1
            2
            4
            8
           16
           32
           64
(7 rows)
```

Refused:

```sql
BEGIN L: LOOP LEAVE L; END LOOP M; END;
BEGIN L: LOOP LEAVE L; END WHILE; END;
BEGIN L: LOOP L: LOOP LEAVE L; END LOOP; END LOOP; END;
```

```
turborel: SQLCODE -103 (SQLSTATE 42000): the <ending label> M is not L [ISO/IEC 9075-4:2006 draft §14.1 SR3] (at character 32)
turborel: SQLCODE -102 (SQLSTATE 42000): expected LOOP, found WHILE (at character 27)
turborel: SQLCODE -103 (SQLSTATE 42000): L is the <beginning label> of a statement this one is in [ISO/IEC 9075-4:2006 draft §14.1 SR4] (at character 14)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.10, Feature P002. SQL-86 and SQL-92 have
no such statement.

## SEE ALSO

[LEAVE](leave.md), [ITERATE](iterate.md), [WHILE](while.md),
[REPEAT](repeat.md), [FOR](for.md), [BEGIN](begin.md)
