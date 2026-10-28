# INSERT

[Statements](README.md)

## NAME

INSERT: insert rows

## SYNOPSIS

```
<insert statement> ::=
    INSERT INTO <table name> <insert columns and source>

<insert columns and source> ::=
      [ ( <column name> [ { , <column name> }... ] ) ] <query expression>
    | DEFAULT VALUES

<row value constructor element> ::= <value expression> | NULL | DEFAULT
```

The `<query expression>` is any query (see [SELECT](select.md)); most
often `VALUES (...), (...)`, each row a list of
`<row value constructor element>`s.

## DESCRIPTION

INSERT adds rows to a base table, or through an updatable view to the
table it is drawn from. The rows are those of its query, each value going
to a column of the column list, in order, or, without one, to each column
of the table in order. A column not given a value takes its default: the
column's, its domain's, or the null value.

In VALUES, DEFAULT gives a column its default and NULL the null value.
DEFAULT VALUES inserts one row of every column's default.

The query is evaluated in full before any row is inserted, so a query
that reads the table inserted into sees none of the rows it inserts. An
INSERT whose query gives no row completes with no data.

## RULES

1. The table exists and is not read-only: a base table, or a view that
   can be updated.
2. Each column listed is a column of it, listed once.
3. The query has as many columns as are inserted into, each value
   assignable to its column's type.
4. Each row inserted satisfies the table's constraints, and, through a
   view WITH CHECK OPTION, is a row of the view.
5. The transaction is not READ ONLY, but for a temporary table.

## PRIVILEGES

INSERT on each column inserted into: each listed, or, without a list,
each of the table's; INSERT granted on the table is on each of its
columns. SELECT on each table the query reads.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The table does not exist or is read-only; a column listed is not of it, or is listed twice; the query's columns are not as many as those inserted into, or a value is not assignable to its column |
| 2A000 | -104 | Its user lacks a privilege it needs |
| 23000 | -201 | A row breaks a constraint: NOT NULL, UNIQUE, PRIMARY KEY, a foreign key or a CHECK |
| 44000 | -201 | A row inserted through a view WITH CHECK OPTION is not a row of the view |
| 22xxx | -301 | A data exception as a value is computed or assigned |
| 25000 | -917 | The transaction is READ ONLY |
| 02000 | 100 | Not an error: the query gave no row |

A statement that fails inserts no row.

## EXAMPLES

```sql
CREATE TABLE PARTS (ID INTEGER NOT NULL PRIMARY KEY, NAME VARCHAR(20) DEFAULT 'unnamed', QTY INTEGER DEFAULT 0);
INSERT INTO PARTS VALUES (1, 'bolt', 100);
INSERT INTO PARTS (ID, NAME) VALUES (2, 'nut'), (3, DEFAULT);
INSERT INTO PARTS (ID) SELECT ID + 10 FROM PARTS;
SELECT * FROM PARTS ORDER BY ID;
```

```
OK
OK
OK
OK
           ID | NAME                 |          QTY
--------------+----------------------+--------------
            1 | bolt                 |          100
            2 | nut                  |            0
            3 | unnamed              |            0
           11 | unnamed              |            0
           12 | unnamed              |            0
           13 | unnamed              |            0
(6 rows)
```

The INSERT ... SELECT read the three rows there before it inserted its
own, so it inserted three.

Rows refused:

```sql
INSERT INTO PARTS VALUES (1, 'again', 1);
INSERT INTO PARTS (NAME) VALUES ('no id');
INSERT INTO PARTS VALUES (5, 'big', 'many');
```

```
turborel: SQLCODE -201 (SQLSTATE 23000): PRIMARY KEY ANN.PARTS_PRIMARY1 (ID) of ANN.PARTS would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
turborel: SQLCODE -201 (SQLSTATE 23000): column ID of ANN.PARTS is NOT NULL [X3.135-1986 §6.3 SR3, GR1]
turborel: SQLCODE -103 (SQLSTATE 2A000): cannot assign a CHARACTER(4) value to column QTY (INTEGER) [ISO/IEC 9075:1992 §13.8 SR6, §9.2 SR2] (at character 36)
```

Through a view WITH CHECK OPTION:

```sql
CREATE VIEW SMALL AS SELECT ID, NAME FROM PARTS WHERE ID < 100 WITH CHECK OPTION;
INSERT INTO SMALL VALUES (500, 'too big');
INSERT INTO SMALL VALUES (50, 'fits');
SELECT ID, NAME, QTY FROM PARTS WHERE ID = 50;
```

```
OK
turborel: SQLCODE -201 (SQLSTATE 44000): the row is not in view ANN.SMALL, defined WITH CASCADED CHECK OPTION [ISO/IEC 9075:1992 §11.19 GR11; X3.135-1986 §8.7 GR2]
OK
           ID | NAME                 |          QTY
--------------+----------------------+--------------
           50 | fits                 |            0
(1 row)
```

## STANDARD

ISO/IEC 9075:1992 §13.8, and X3.135-1986 §8.7, whose INSERT takes
`VALUES` of one row or a query specification. DEFAULT VALUES, a
`<query expression>` of any kind, and NULL and DEFAULT in VALUES are
SQL-92's [§13.8, §7.1].

## SEE ALSO

[UPDATE](update.md), [DELETE](delete.md), [SELECT](select.md),
[CREATE TABLE](create-table.md), [Changing Data](../../user/05-changing-data.md)
