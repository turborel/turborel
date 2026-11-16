# PIPE ROW

[Statements](README.md)

## NAME

PIPE ROW: give a row of a polymorphic table function's result *(in a component procedure)*

## SYNOPSIS

```
<pipe row statement> ::= PIPE ROW <PTF descriptor name>

<PTF descriptor name> ::= PTF <simple value specification>
```

PIPE and ROW are read as words, not key words.

## DESCRIPTION

PIPE ROW gives the result of the polymorphic table function whose
start, fulfill or finish component procedure runs it a row. The row is
the DATA of each item of the intermediate result row type's descriptor
area, which the component is passed the name of: the proper result
columns, then of each table parameter of PASS THROUGH a surrogate.

A surrogate is a value the component read by [FETCH](fetch.md) from its
table's cursor, its last column. The result row then has that table's
columns of the row the surrogate names. A null surrogate gives null
columns. The row's partitioning columns are the partition's values.

## RULES

1. A start, fulfill or finish component procedure runs it.
2. The name is of the intermediate result row type's area.
3. Each surrogate is null or one the table's cursor gave.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 0700H | -921 | No start, fulfill or finish component runs it (rule 1) |
| 33000 | -921 | The name is of another area (rule 2) |
| 0700G | -921 | A surrogate names no row of its table (rule 3) |

## EXAMPLES

A fulfill component that gives each row of its table whose N is over
15, of a function of RETURNS ONLY PASS THROUGH:

```sql
CREATE TABLE T (K INTEGER, N INTEGER);
INSERT INTO T VALUES (1, 10), (2, 20), (3, 30);
CREATE PROCEDURE OVER_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5))
  CONTAINS SQL
BEGIN
  DECLARE K, N INTEGER;
  DECLARE S BIGINT;
  DECLARE DONE BOOLEAN DEFAULT FALSE;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE;
  L: LOOP
    FETCH NEXT FROM PTF CU INTO K, N, S;
    IF DONE THEN LEAVE L; END IF;
    IF N > 15 THEN
      SET DESCRIPTOR PTF RR VALUE 1 DATA = S;
      PIPE ROW PTF RR;
    END IF;
  END LOOP L;
END;
CREATE FUNCTION OVER (T TABLE PASS THROUGH WITH ROW SEMANTICS) RETURNS ONLY PASS THROUGH
  CONTAINS SQL FULFILL WITH PROCEDURE OVER_F;
SELECT * FROM TABLE (OVER (TABLE (T))) ORDER BY K;
```

```
OK
OK
OK
OK
            K |            N
--------------+--------------
            2 |           20
            3 |           30
(2 rows)
```

Run where no component runs:

```sql
CREATE PROCEDURE LOOSE (IN RR VARCHAR(128)) CONTAINS SQL PIPE ROW PTF RR;
CALL LOOSE ('X');
```

```
OK
turborel: SQLCODE -921 (SQLSTATE 0700H): PIPE ROW is executed by a PTF start, fulfill or finish component procedure alone [ISO/IEC 9075-2:2023 §20.28 GR1]
```

## STANDARD

ISO/IEC 9075-2:2023 §20.28, Feature B209. SQL-86 and SQL-92 have no
PIPE ROW.

## SEE ALSO

[CREATE FUNCTION](create-function.md), [FETCH](fetch.md),
[SET DESCRIPTOR](set-descriptor.md), [COPY DESCRIPTOR](copy-descriptor.md)
