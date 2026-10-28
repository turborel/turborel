# Retrieving Data

[Contents](README.md)

The examples are of the tables of [Getting Started](02-getting-started.md).

## The query specification

A query says what to select, from which tables, of which rows:

```sql
SELECT NAME, SALARY FROM EMPLOYEES WHERE DEPT = 'ENG' ORDER BY SALARY DESC;
```

SELECT names the columns of the result, or `*` for all; FROM the
tables; WHERE the condition each row kept meets
([Predicates](../sql/05-predicates.md)). GROUP BY makes a row of each
group, of set functions such as COUNT and AVG, and HAVING keeps the
groups its condition is true of ([Query Expressions](../sql/06-queries.md)).

## Joins

A join pairs the rows of two tables: `A JOIN B ON condition` the pairs
the condition is true of; LEFT, RIGHT and FULL joins also keep the rows
of one or both that pair with none, the other's columns null.

```sql
SELECT D.NAME, COUNT(*) AS STAFF, AVG(E.SALARY) AS AVERAGE
  FROM DEPARTMENTS D JOIN EMPLOYEES E ON E.DEPT = D.CODE
 GROUP BY D.NAME ORDER BY D.NAME;
```

## Subqueries

A query in parentheses may stand for a value, its one row's, or for a
set of rows, with IN, EXISTS, ANY and ALL:

```sql
SELECT NAME FROM EMPLOYEES WHERE SALARY > (SELECT AVG(SALARY) FROM EMPLOYEES);
SELECT NAME FROM DEPARTMENTS D
 WHERE NOT EXISTS (SELECT * FROM EMPLOYEES E WHERE E.DEPT = D.CODE AND E.SALARY > 6000);
SELECT NAME FROM EMPLOYEES WHERE DEPT IN (SELECT CODE FROM DEPARTMENTS WHERE NAME LIKE 'Eng%') ORDER BY NAME;
```

```
 NAME
--------------------------------
 Ada
 Grace
(2 rows)
 NAME
--------------------------------
 Operations
(1 row)
 NAME
--------------------------------
 Ada
 Grace
(2 rows)
```

The second subquery reads D.CODE, a column of the query around it: it
is found again for each department.

## Set operations

UNION, EXCEPT and INTERSECT combine the rows of two queries of the same
columns:

```sql
SELECT CODE FROM DEPARTMENTS EXCEPT SELECT DEPT FROM EMPLOYEES WHERE SALARY < 6000;
```

```

-----
 ENG
(1 row)
```

## Ordering

Without ORDER BY, rows come in no order to count on. ORDER BY sorts by
columns of the result, each ascending or DESC, nulls last ascending:

```sql
SELECT DEPT, NAME, SALARY FROM EMPLOYEES ORDER BY DEPT, SALARY DESC;
```

```
 DEPT | NAME                           |      SALARY
------+--------------------------------+-------------
 ENG  | Grace                          |     8100.00
 ENG  | Ada                            |     7200.00
 OPS  | Edsger                         |     5400.00
(3 rows)
```

A sort key is a column the query selects: ORDER BY DEPT of a query
that does not select DEPT is refused.
