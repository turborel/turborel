# LEAVE

[Statements](README.md)

## NAME

LEAVE: end a labelled statement

## SYNOPSIS

```
<leave statement> ::= LEAVE <statement label>
```

LEAVE is read as a word, not a key word. A label is an identifier.

## DESCRIPTION

LEAVE ends the labelled statement it names, a compound statement, LOOP,
WHILE, REPEAT or FOR that LEAVE is in. Control goes on after it. Each
statement between LEAVE and the one it names ends too, and the cursors
of each compound statement it leaves are destroyed.

LEAVE stands within the statement it names, in a routine's body, a trigger's action or a
compound statement run directly.

## RULES

1. The label is that of a statement LEAVE is in.
2. In a handler's action, the label is not that of the handler's
   compound statement.
3. In a FOR, the label is not the FOR's loop variable name.
4. LEAVE is not preparable.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |

## EXAMPLES

LEAVE out of two compound statements at once, and out of a loop:

```sql
CREATE TABLE TRAIL (STEP INTEGER);
BEGIN
  DECLARE I INTEGER DEFAULT 0;
  OUTSIDE: BEGIN
    INSERT INTO TRAIL VALUES (1);
    INSIDE: BEGIN
      INSERT INTO TRAIL VALUES (2);
      LEAVE OUTSIDE;
      INSERT INTO TRAIL VALUES (3);
    END INSIDE;
    INSERT INTO TRAIL VALUES (4);
  END OUTSIDE;
  SCAN: LOOP
    SET I = I + 1;
    IF I > 3 THEN LEAVE SCAN; END IF;
    INSERT INTO TRAIL VALUES (10 + I);
  END LOOP SCAN;
END;
SELECT STEP FROM TRAIL ORDER BY STEP;
```

```
OK
OK
         STEP
--------------
            1
            2
           11
           12
           13
(5 rows)
```

Refused:

```sql
BEGIN LEAVE NOWHERE; END;
B: BEGIN DECLARE EXIT HANDLER FOR SQLEXCEPTION LEAVE B; SIGNAL SQLSTATE '75001'; END B;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): LEAVE NOWHERE names no statement it is in [ISO/IEC 9075-4:2006 draft §14.9 SR1] (at character 12)
turborel: SQLCODE -103 (SQLSTATE 2A000): LEAVE B names no statement it is in [ISO/IEC 9075-4:2006 draft §14.9 SR1] (at character 53)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.9, Feature P002. SQL-86 and SQL-92 have no
such statement.

## SEE ALSO

[ITERATE](iterate.md), [LOOP](loop.md), [WHILE](while.md),
[REPEAT](repeat.md), [FOR](for.md), [BEGIN](begin.md)
