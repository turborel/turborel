# COPY DESCRIPTOR

[Statements](README.md)

## NAME

COPY DESCRIPTOR: copy a descriptor area, or an item's fields, to a PTF descriptor area *(in a component procedure)*

## SYNOPSIS

```
<copy descriptor statement> ::=
      COPY <descriptor name> TO <PTF descriptor name>
    | COPY <descriptor name> VALUE <item number>
        ( { NAME | TYPE | NAME , TYPE | DATA } )
        TO <PTF descriptor name> VALUE <item number>

<PTF descriptor name> ::= PTF <simple value specification>
```

COPY is read as a word, not a key word.

## DESCRIPTION

COPY DESCRIPTOR, in a component procedure of a polymorphic table
function ([CREATE FUNCTION](create-function.md)), copies from one of
its PTF descriptor areas to another.

Of the whole area, every item and COUNT are copied. Of an item, of LEVEL
0, the fields named are copied to the target item. NAME copies its
name. TYPE copies its type, and of a row or an array the items of its
parts after it. DATA copies its value. A target item past the target's
COUNT makes COUNT reach it.

So a describe component may take a column of a table argument's full row
type for its requested row type, or for the result's.

## RULES

1. Each name is of an area the component's PTF data area has.
2. The item numbers are exact numeric with scale 0. The source's is of
   an item of LEVEL 0. The target's is from 1 to its area's occurrences,
   and of an item of LEVEL 0 if within its COUNT.
3. The source fits in the target.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 33000 | -921 | A name is of no area (rule 1) |
| 07009 | -921 | An item number is of no item, or not of LEVEL 0, or the source does not fit (rules 2, 3) |
| 42000 | -103 | The target is no `<PTF descriptor name>`, or an item number is not exact with scale 0 |

## EXAMPLES

A describe component that reads the column a descriptor argument names
and gives the result one column of its type, and a fulfill component
that gives its values:

```sql
CREATE TABLE T (K INTEGER, N INTEGER, M INTEGER);
INSERT INTO T VALUES (1, 10, 100), (2, 20, 200);
CREATE PROCEDURE PICK_D (IN FR VARCHAR(128), IN RQ VARCHAR(128), IN CD VARCHAR(128),
                         IN IR VARCHAR(128), INOUT ST CHAR(5))
  CONTAINS SQL DETERMINISTIC
BEGIN
  DECLARE C, N, I INTEGER;
  DECLARE NM, WANTED VARCHAR(128);
  GET DESCRIPTOR PTF CD VALUE 1 WANTED = NAME;
  GET DESCRIPTOR PTF FR C = COUNT;
  SET I = 1;
  WHILE I <= C DO
    GET DESCRIPTOR PTF FR VALUE I NM = NAME;
    IF NM = WANTED THEN SET N = I; END IF;
    SET I = I + 1;
  END WHILE;
  IF N IS NULL THEN
    SET ST = '22023';
  ELSE
    COPY PTF FR VALUE N (NAME, TYPE) TO PTF RQ VALUE 1;
    SET DESCRIPTOR PTF IR COUNT = 1;
    COPY PTF FR VALUE N (TYPE) TO PTF IR VALUE 1;
    SET DESCRIPTOR PTF IR VALUE 1 NAME = 'PICKED';
  END IF;
END;
CREATE PROCEDURE PICK_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN CD VARCHAR(128),
                         IN RR VARCHAR(128), INOUT ST CHAR(5))
  CONTAINS SQL
BEGIN
  DECLARE V INTEGER;
  DECLARE DONE BOOLEAN DEFAULT FALSE;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE;
  L: LOOP
    FETCH NEXT FROM PTF CU INTO V;
    IF DONE THEN LEAVE L; END IF;
    SET DESCRIPTOR PTF RR VALUE 1 DATA = V;
    PIPE ROW PTF RR;
  END LOOP L;
END;
CREATE FUNCTION PICK (T TABLE WITH ROW SEMANTICS, C DESCRIPTOR) RETURNS TABLE CONTAINS SQL
  DESCRIBE WITH PROCEDURE PICK_D FULFILL WITH PROCEDURE PICK_F;
SELECT * FROM TABLE (PICK (TABLE (T), DESCRIPTOR (M))) AS P ORDER BY 1;
SELECT * FROM TABLE (PICK (TABLE (T), DESCRIPTOR (Z))) AS P;
```

```
OK
OK
OK
OK
OK
       PICKED
--------------
          100
          200
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 42000): the PTF describe component procedure left the status variable '22023' [ISO/IEC 9075-2:2023 §9.18 SR12j] (at character 21)
```

## STANDARD

ISO/IEC 9075-2:2023 §20.6, Feature B209. SQL-86 and SQL-92 have no COPY
DESCRIPTOR.

## SEE ALSO

[CREATE FUNCTION](create-function.md), [GET DESCRIPTOR](get-descriptor.md),
[SET DESCRIPTOR](set-descriptor.md), [PIPE ROW](pipe-row.md)
