# ITERATE

[Statements](README.md)

## NAME

ITERATE: begin the next iteration of a loop *(in a labelled statement)*

## SYNOPSIS

```
<iterate statement> ::= ITERATE <statement label>
```

ITERATE is read as a word, not a key word. A label is an identifier.

## DESCRIPTION

ITERATE ends the current iteration of the LOOP, WHILE, REPEAT or FOR it
names, skipping the rest of its statements. LOOP then runs its
statements again. WHILE reads its condition, and REPEAT its UNTIL
condition, as each would after an iteration. FOR goes on to its next
row. Each statement between ITERATE and the loop it names ends, and
the cursors of each compound statement it leaves are destroyed.

ITERATE stands within the loop it names, in a routine's body, a
trigger's action or a compound statement run directly.

## RULES

1. The label is that of a LOOP, WHILE, REPEAT or FOR that ITERATE is
   in, not of a compound statement.
2. ITERATE is not preparable.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |

## EXAMPLES

The even numbers skipped:

```sql
CREATE TABLE ODDS (N INTEGER);
BEGIN
  DECLARE I INTEGER DEFAULT 0;
  WALK: WHILE I < 9 DO
    SET I = I + 1;
    IF I IN (2, 4, 6, 8) THEN ITERATE WALK; END IF;
    INSERT INTO ODDS VALUES (I);
  END WHILE WALK;
END;
SELECT N FROM ODDS ORDER BY N;
```

```
OK
OK
            N
--------------
            1
            3
            5
            7
            9
(5 rows)
```

Refused:

```sql
B: BEGIN ITERATE B; END B;
BEGIN ITERATE NOWHERE; END;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ITERATE B names no iterated statement it is in [ISO/IEC 9075-4:2006 draft §14.8 SR1] (at character 17)
turborel: SQLCODE -103 (SQLSTATE 2A000): ITERATE NOWHERE names no iterated statement it is in [ISO/IEC 9075-4:2006 draft §14.8 SR1] (at character 14)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.8, Feature P002. SQL-86 and SQL-92 have no
such statement.

## SEE ALSO

[LEAVE](leave.md), [LOOP](loop.md), [WHILE](while.md),
[REPEAT](repeat.md), [FOR](for.md)
