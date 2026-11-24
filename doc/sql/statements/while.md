# WHILE

[Statements](README.md)

## NAME

WHILE: run statements while a condition is true

## SYNOPSIS

```
<while statement> ::=
    [ <beginning label> : ] WHILE <search condition> DO
    <statement list>
    END WHILE [ <ending label> ]

<statement list> ::= { <SQL procedure statement> ; }...
```

END is a key word. WHILE and DO are read as words, not key words. A
label is an identifier.

## DESCRIPTION

WHILE reads its condition before each iteration. If it is true, the
statements run and WHILE begins again. If it is false or unknown, WHILE
ends. So a condition false at first runs no statement.

[LEAVE](leave.md) of its label ends it at once, and
[ITERATE](iterate.md) of its label goes on to read the condition
again.

WHILE stands in a [compound statement](begin.md), a routine's body or a
trigger's action, and may be run directly too.

A loop whose condition stays true is cancelled as any statement is,
with SQLCODE -920, and the session goes on.

## RULES

1. An ending label is the loop's beginning label.
2. A beginning label is not the label of a statement the loop is in.
3. The list holds one statement or more, each ended by a semicolon.
4. WHILE is not preparable.

## PRIVILEGES

Those of its condition and statements.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | It is not well formed |
| 42000 | -103 | A rule above is broken |
| TB920 | -920 | It is cancelled |

## EXAMPLES

```sql
CREATE TABLE SQUARES (N INTEGER, SQ INTEGER);
BEGIN
  DECLARE I INTEGER DEFAULT 1;
  WHILE I <= 5 DO
    INSERT INTO SQUARES VALUES (I, I * I);
    SET I = I + 1;
  END WHILE;
  WHILE I > 100 DO
    INSERT INTO SQUARES VALUES (0, 0);
  END WHILE;
END;
SELECT N, SQ FROM SQUARES ORDER BY N;
```

```
OK
OK
            N |           SQ
--------------+--------------
            1 |            1
            2 |            4
            3 |            9
            4 |           16
            5 |           25
(5 rows)
```

A procedure that fills a table up to its argument, and a label that
does not match:

```sql
CREATE TABLE SLOTS (N INTEGER);
CREATE PROCEDURE FILL_SLOTS (IN UPTO INTEGER) MODIFIES SQL DATA
BEGIN
  DECLARE I INTEGER DEFAULT 1;
  FILLING: WHILE I <= UPTO DO
    INSERT INTO SLOTS VALUES (I);
    SET I = I + 1;
  END WHILE FILLING;
END;
CALL FILL_SLOTS(3);
SELECT N FROM SLOTS ORDER BY N;
BEGIN DECLARE X INTEGER; W1: WHILE FALSE DO SET X = 1; END WHILE W2; END;
```

```
OK
OK
OK
            N
--------------
            1
            2
            3
(3 rows)
turborel: SQLCODE -103 (SQLSTATE 42000): the <ending label> W2 is not W1 [ISO/IEC 9075-4:2006 draft §14.1 SR3] (at character 65)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.11, Feature P002. SQL-86 and SQL-92 have
no such statement.

## SEE ALSO

[LOOP](loop.md), [REPEAT](repeat.md), [FOR](for.md),
[LEAVE](leave.md), [ITERATE](iterate.md), [BEGIN](begin.md)
