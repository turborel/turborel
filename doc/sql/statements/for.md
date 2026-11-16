# FOR

[Statements](README.md)

## NAME

FOR: run statements for each row of a query

## SYNOPSIS

```
<for statement> ::=
    [ <beginning label> : ] FOR [ <for loop variable name> AS ]
    [ <cursor name> [ SENSITIVE | INSENSITIVE | ASENSITIVE ] CURSOR FOR ]
    <cursor specification>
    DO <statement list>
    END FOR [ <ending label> ]

<statement list> ::= { <SQL procedure statement> ; }...
```

FOR, AS, INSENSITIVE, CURSOR and END are key words. DO, SENSITIVE and
ASENSITIVE are read as words, not key words. A label and a loop
variable name are identifiers.

## DESCRIPTION

FOR opens a cursor of its query and, for each row the cursor fetches,
runs its statements. Each column of the query is a variable of the
FOR, named by the column's name, which the statements read. A column
that is an SQL parameter or SQL variable alone is named for it. The loop
variable name, if written, qualifies them, so `R.QTY` is the column QTY
of the FOR named R. The cursor is closed and destroyed as FOR ends.

A label names the FOR, which [LEAVE](leave.md) ends and
[ITERATE](iterate.md) moves on to the next row. A cursor name names the
cursor, which a positioned UPDATE or DELETE in the statements may
name.

FOR stands in a [compound statement](begin.md), a routine's body or a
trigger's action, and may be run directly too.

## RULES

1. Each column of the query has a name, no two the same.
2. An ending label is the FOR's beginning label. A beginning label is
   not the label of a statement the FOR is in.
3. LEAVE does not name the loop variable name. It names the label.
4. The statements do not OPEN, FETCH from or CLOSE the FOR's cursor.
5. The statements hold no COMMIT or ROLLBACK.
6. The list holds one statement or more, each ended by a semicolon.
7. FOR is not preparable.

## PRIVILEGES

SELECT on what the query reads, and those of the statements.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | It is not well formed |
| 2A000 | -103 | A rule above is broken |
| TB920 | -920 | It is cancelled |

## EXAMPLES

Each row low in stock reordered:

```sql
CREATE TABLE STOCK (ITEM VARCHAR(10), QTY INTEGER);
CREATE TABLE REORDER (ITEM VARCHAR(10), NEEDED INTEGER);
INSERT INTO STOCK VALUES ('bolt', 3), ('nut', 40), ('washer', 0);
BEGIN
  FOR R AS SELECT ITEM, QTY FROM STOCK DO
    IF R.QTY < 10 THEN
      INSERT INTO REORDER VALUES (R.ITEM, 10 - R.QTY);
    END IF;
  END FOR;
END;
SELECT ITEM, NEEDED FROM REORDER ORDER BY ITEM;
```

```
OK
OK
OK
OK
 ITEM       |       NEEDED
------------+--------------
 bolt       |            7
 washer     |           10
(2 rows)
```

A labelled FOR left part way, its cursor named:

```sql
BEGIN
  DECLARE SEEN INTEGER DEFAULT 0;
  SCAN: FOR S AS C1 CURSOR FOR SELECT QTY FROM STOCK ORDER BY QTY DO
    SET SEEN = SEEN + 1;
    IF QTY > 5 THEN LEAVE SCAN; END IF;
  END FOR SCAN;
  INSERT INTO REORDER VALUES ('seen', SEEN);
END;
SELECT ITEM, NEEDED FROM REORDER WHERE ITEM = 'seen';
```

```
OK
 ITEM       |       NEEDED
------------+--------------
 seen       |            3
(1 row)
```

A procedure's parameter in the query names its column, so the row
reads it as `R.MINIMUM`:

```sql
CREATE PROCEDURE SHORT (IN MINIMUM INTEGER) MODIFIES SQL DATA
BEGIN
  FOR R AS SELECT ITEM, QTY, MINIMUM FROM STOCK DO
    IF R.QTY < R.MINIMUM THEN
      INSERT INTO REORDER VALUES (R.ITEM, R.MINIMUM - R.QTY);
    END IF;
  END FOR;
END;
DELETE FROM REORDER;
CALL SHORT(5);
SELECT ITEM, NEEDED FROM REORDER ORDER BY ITEM;
```

```
OK
OK
OK
 ITEM       |       NEEDED
------------+--------------
 bolt       |            2
 washer     |            5
(2 rows)
```

Rules refused:

```sql
BEGIN FOR S AS SELECT QTY FROM STOCK DO LEAVE S; END FOR; END;
BEGIN FOR S AS C2 CURSOR FOR SELECT QTY FROM STOCK DO CLOSE C2; END FOR; END;
BEGIN FOR SELECT QTY + 1 FROM STOCK DO INSERT INTO REORDER VALUES ('x', 1); END FOR; END;
BEGIN FOR SELECT QTY FROM STOCK DO COMMIT; END FOR; END;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): the statements of a FOR shall not LEAVE its <for loop variable name> S [ISO/IEC 9075-4:2006 draft §14.13 SR8] (at character 46)
turborel: SQLCODE -103 (SQLSTATE 2A000): the statements of a FOR shall not OPEN, FETCH or CLOSE its cursor C2 [ISO/IEC 9075-4:2006 draft §14.13 SR10] (at character 54)
turborel: SQLCODE -103 (SQLSTATE 2A000): column 1 of the FOR statement's query has no name of its own [ISO/IEC 9075-4:2006 draft §14.13 SR5] (at character 6)
turborel: SQLCODE -103 (SQLSTATE 2A000): an ATOMIC compound statement or a FOR holds no COMMIT or ROLLBACK [ISO/IEC 9075-4:2006 draft §14.1 SR6, §14.13 SR9] (at character 35)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.13, Feature P002. SQL-86 and SQL-92 have
no such statement.

## SEE ALSO

[DECLARE CURSOR](declare-cursor.md), [LOOP](loop.md),
[WHILE](while.md), [LEAVE](leave.md), [ITERATE](iterate.md),
[BEGIN](begin.md)
