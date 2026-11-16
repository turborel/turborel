# CASE (statement)

[Statements](README.md)

## NAME

CASE: run the statements of the first WHEN that holds

## SYNOPSIS

```
<case statement> ::=
      CASE <case operand>
          { WHEN <when operand> [ , <when operand> ]... THEN <statement list> }...
          [ ELSE <statement list> ]
          END CASE
    | CASE
          { WHEN <search condition> THEN <statement list> }...
          [ ELSE <statement list> ]
          END CASE

<when operand> ::=
      <row value>
    | <comp op> <row value>
    | IS [ NOT ] NULL
    | [ NOT ] BETWEEN <row value> AND <row value>
    | [ NOT ] IN ( <value list> )
    | [ NOT ] LIKE <pattern>
    | IS [ NOT ] DISTINCT FROM <row value>

<statement list> ::= { <SQL procedure statement> ; }...
```

CASE, WHEN, THEN, ELSE and END are key words.

## DESCRIPTION

The CASE statement chooses a list of statements to run. It stands in a
[compound statement](begin.md), a routine's body or a trigger's action,
and may be run directly too.

A searched CASE runs the statements of its first WHEN whose condition
is true. A simple CASE compares its operand with each WHEN's operands,
and runs the statements of the first WHEN one of whose operands holds.
A WHEN operand that is a value holds if it equals the case operand. One
that begins with a comparison or a predicate, such as `> 5` or
`IS NULL`, holds if that predicate of the case operand is true. The
operands may be rows, such as `CASE (X, Y) WHEN (1, 2) THEN`.

If no WHEN holds, the statements of ELSE run. A CASE statement with no
ELSE, none of whose WHEN holds, raises SQLSTATE 20000. A condition that
is unknown does not hold.

The CASE statement is not the CASE expression, which gives a value and
ends with END alone.

## RULES

1. Each list holds one statement or more, each ended by a semicolon.
2. The case operand invokes no function that possibly modifies
   SQL-data.
3. Each WHEN operand is comparable with the case operand, of its
   degree.
4. A CASE statement takes no label, and is not preparable.

## PRIVILEGES

Those of its operands, conditions and statements.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | It is not well formed, as a CASE ended by END IF |
| 2A000 | -103 | A rule above is broken |
| 20000 | -301 | No WHEN holds and there is no ELSE |

## EXAMPLES

A searched CASE in a procedure:

```sql
CREATE TABLE GRADES (SCORE INTEGER, MARK CHAR(1));
CREATE PROCEDURE GRADE_IT (IN S INTEGER) MODIFIES SQL DATA
BEGIN
  CASE
    WHEN S >= 90 THEN INSERT INTO GRADES VALUES (S, 'A');
    WHEN S >= 75 THEN INSERT INTO GRADES VALUES (S, 'B');
    ELSE INSERT INTO GRADES VALUES (S, 'C');
  END CASE;
END;
CALL GRADE_IT(95);
CALL GRADE_IT(80);
CALL GRADE_IT(40);
SELECT SCORE, MARK FROM GRADES ORDER BY SCORE;
```

```
OK
OK
OK
OK
OK
        SCORE | MARK
--------------+------
           40 | C
           80 | B
           95 | A
(3 rows)
```

A simple CASE of lists of operands, a predicate and rows:

```sql
CREATE TABLE WEEK (N INTEGER, KIND VARCHAR(10));
BEGIN
  DECLARE D INTEGER DEFAULT 1;
  WHILE D <= 7 DO
    CASE D
      WHEN 6, 7 THEN INSERT INTO WEEK VALUES (D, 'weekend');
      WHEN > 7 THEN SIGNAL SQLSTATE '75001';
      ELSE INSERT INTO WEEK VALUES (D, 'weekday');
    END CASE;
    SET D = D + 1;
  END WHILE;
  CASE (D, 0) WHEN (8, 0) THEN INSERT INTO WEEK VALUES (D, 'done'); END CASE;
END;
SELECT N, KIND FROM WEEK ORDER BY N;
```

```
OK
OK
            N | KIND
--------------+------------
            1 | weekday
            2 | weekday
            3 | weekday
            4 | weekday
            5 | weekday
            6 | weekend
            7 | weekend
            8 | done
(8 rows)
```

No WHEN holds, and a CASE ended wrongly:

```sql
BEGIN
  DECLARE X INTEGER DEFAULT 4;
  CASE X WHEN 1 THEN SET X = 0; WHEN 2 THEN SET X = 0; END CASE;
END;
BEGIN
  DECLARE X INTEGER DEFAULT 4;
  CASE X WHEN 1 THEN SET X = 0; END IF;
END;
```

```
turborel: SQLCODE -301 (SQLSTATE 20000): no WHEN of the CASE statement is true, and it has no ELSE [ISO/IEC 9075-4:2006 draft §14.6 GR1b, GR2b] (at character 39)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected CASE, found IF (at character 73)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.6, Feature P002. Row operands and WHEN
operands that are predicates are Feature P004, lists of WHEN operands
Feature P008. SQL-86 and SQL-92 have no such statement.

## SEE ALSO

[IF](if.md), [BEGIN](begin.md), [SELECT](select.md)
