# IF

[Statements](README.md)

## NAME

IF: run statements if a condition is true

## SYNOPSIS

```
<if statement> ::=
    IF <search condition> THEN <statement list>
    [ ELSEIF <search condition> THEN <statement list> ]...
    [ ELSE <statement list> ]
    END IF

<statement list> ::= { <SQL procedure statement> ; }...
```

THEN, ELSE and END are key words. IF and ELSEIF are read as words, not
key words.

## DESCRIPTION

IF runs the statements of the first condition that is true, IF's or an
ELSEIF's, else those of ELSE. A condition that is false or unknown is
not true. With none true and no ELSE, IF does nothing.

IF stands in a [compound statement](begin.md), a routine's body or a
trigger's action, and may be run directly too.

## RULES

1. Each list holds one statement or more, each ended by a semicolon.
2. IF takes no label, and is not preparable.

## PRIVILEGES

Those of its conditions and statements.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | It is not well formed, as a list of no statement |
| 42000 | -103 | It has a label |

## EXAMPLES

A null argument is neither small nor medium, so ELSE runs:

```sql
CREATE TABLE SIZES (V INTEGER, KIND VARCHAR(10));
CREATE PROCEDURE CLASSIFY (IN N INTEGER) MODIFIES SQL DATA
BEGIN
  IF N < 10 THEN
    INSERT INTO SIZES VALUES (N, 'small');
  ELSEIF N < 100 THEN
    INSERT INTO SIZES VALUES (N, 'medium');
  ELSE
    INSERT INTO SIZES VALUES (N, 'large');
  END IF;
  IF N IS NULL THEN
    INSERT INTO SIZES VALUES (N, 'unknown');
  END IF;
END;
CALL CLASSIFY(5);
CALL CLASSIFY(50);
CALL CLASSIFY(500);
CALL CLASSIFY(CAST(NULL AS INTEGER));
SELECT V, KIND FROM SIZES ORDER BY KIND;
```

```
OK
OK
OK
OK
OK
OK
            V | KIND
--------------+------------
          500 | large
         NULL | large
           50 | medium
            5 | small
         NULL | unknown
(5 rows)
```

Refused:

```sql
BEGIN IF 1 = 1 THEN END IF; END;
BEGIN L: IF 1 = 1 THEN INSERT INTO SIZES VALUES (0, 'x'); END IF; END;
```

```
turborel: SQLCODE -102 (SQLSTATE 42000): expected an <SQL procedure statement>, found END (at character 20)
turborel: SQLCODE -103 (SQLSTATE 42000): this statement has no <beginning label> [ISO/IEC 9075-4:2006 draft §14.1] (at character 6)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.7, Feature P002. SQL-86 and SQL-92 have no
such statement.

## SEE ALSO

[CASE](case-statement.md), [BEGIN](begin.md)
