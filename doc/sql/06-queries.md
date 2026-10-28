# Query Expressions

[Contents](README.md)

A query expression gives a table: of rows from tables, joined, filtered,
grouped and selected, or of rows written out, and of those combined by
set operations. [SELECT](statements/select.md) gives the syntax whole;
this chapter its parts. The examples are of STAFF, of [Value
Expressions](04-expressions.md), and of a table DEPT:

```sql
CREATE TABLE DEPT (DEPT CHAR(3) PRIMARY KEY, TITLE VARCHAR(20));
INSERT INTO DEPT VALUES ('ENG', 'Engineering'), ('HR', 'People');
```

## Table references and joins

The FROM clause names the tables a query reads. Of more than one, it
reads every combination of their rows, each row of one with each of the
other, which WHERE then filters. A table may be given a **correlation
name**, `STAFF S`, which then names it in the query.

A **derived table** is a query in parentheses, given a correlation name
and, if written, names for its columns: `(SELECT ...) AS C (D, N)`.

A **joined table** joins two tables in the FROM clause:

| Join | Rows |
|---|---|
| `A [INNER] JOIN B ON c` | each pair of rows for which c is true |
| `A LEFT [OUTER] JOIN B ON c` | those, and each row of A that pairs with none, B's columns null |
| `A RIGHT [OUTER] JOIN B ON c` | those, and each row of B that pairs with none |
| `A FULL [OUTER] JOIN B ON c` | those, and each row of either that pairs with none |
| `A CROSS JOIN B` | every pair |
| `A UNION JOIN B` | each row of A, B's columns null, and each of B, A's null |

`USING (c, ...)` joins on the columns named, equal in both; NATURAL on
every column of one name in both. Either gives each such column once.
Joins nest, and may be written in parentheses.

```sql
SELECT S.NAME, D.TITLE FROM STAFF S, DEPT D WHERE S.DEPT = D.DEPT ORDER BY S.NAME;
SELECT NAME, TITLE FROM STAFF NATURAL LEFT JOIN DEPT ORDER BY NAME;
SELECT NAME, TITLE FROM STAFF FULL JOIN DEPT ON STAFF.DEPT = DEPT.DEPT ORDER BY NAME, TITLE;
SELECT * FROM (SELECT DEPT, COUNT(*) FROM STAFF GROUP BY DEPT) AS C (D, N) WHERE N > 1;
```

```
 NAME                           | TITLE
--------------------------------+----------------------
 Ada                            | Engineering
 Grace                          | Engineering
(2 rows)
 NAME                           | TITLE
--------------------------------+----------------------
 Ada                            | Engineering
 Barbara                        | NULL
 Edsger                         | NULL
 Grace                          | Engineering
(4 rows)
 NAME                           | TITLE
--------------------------------+----------------------
 Ada                            | Engineering
 Barbara                        | NULL
 Edsger                         | NULL
 Grace                          | Engineering
 NULL                           | People
(5 rows)
 D   |                    N
-----+----------------------
 ENG |                    2
(1 row)
```

## The WHERE, GROUP BY and HAVING clauses

**WHERE** keeps the rows its condition is true of.

**GROUP BY** makes groups of the rows, one of each value of its columns,
the nulls one group. A query then gives a row of each group: what it
selects is a grouping column, or of a set function, computed of the
group. **HAVING** keeps the groups its condition is true of; without
GROUP BY, it makes the whole table one group.

```sql
SELECT DEPT, COUNT(*) AS N, SUM(SALARY) AS TOTAL FROM STAFF
    WHERE SALARY IS NOT NULL GROUP BY DEPT HAVING COUNT(*) > 1;
SELECT DEPT, NAME FROM STAFF GROUP BY DEPT;
```

```
 DEPT |                    N |                TOTAL
------+----------------------+----------------------
 ENG  |                    2 |             15300.00
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): outside a set function, a grouped query may only reference grouping columns (found STAFF.NAME) [X3.135-1986 §5.25 SR7] (at character 13)
```

## The select list

The select list says the columns of the result: `*`, every column of
the FROM clause's tables in order; `S.*`, every column of one; or value
expressions, each named by `AS` or, if a column, as it. DISTINCT keeps
one of each row alike, nulls alike; ALL, the default, every row.

```sql
SELECT DISTINCT DEPT FROM STAFF ORDER BY DEPT;
SELECT S.*, D.TITLE FROM STAFF S, DEPT D WHERE S.DEPT = D.DEPT AND S.ID = 1;
SELECT NAME AS WHO, SALARY / 12 AS MONTHLY FROM STAFF WHERE ID = 1;
```

```
 DEPT
------
 ENG
 OPS
 NULL
(3 rows)
           ID | NAME                           | DEPT |      SALARY | TITLE
--------------+--------------------------------+------+-------------+----------------------
            1 | Ada                            | ENG  |     7200.00 | Engineering
(1 row)
 WHO                            |              MONTHLY
--------------------------------+----------------------
 Ada                            |               600.00
(1 row)
```

ORDER BY sorts by columns of the result alone, named or by position: a
column the query does not select is not a sort key.

## Set operations

UNION gives the rows of either query, EXCEPT those of the first not in
the second, INTERSECT those of both; each once, unless ALL keeps each
as often as the operation gives it. INTERSECT binds tighter than UNION
and EXCEPT. The two queries are of one degree, their columns paired by
position, of comparable types; CORRESPONDING pairs them by name instead,
of the columns both have, or those of BY (...).

```sql
SELECT DEPT FROM STAFF UNION SELECT DEPT FROM DEPT ORDER BY 1;
SELECT DEPT FROM DEPT EXCEPT SELECT DEPT FROM STAFF;
SELECT ID, DEPT FROM STAFF UNION CORRESPONDING SELECT DEPT, TITLE FROM DEPT ORDER BY 1;
SELECT ID FROM STAFF UNION SELECT TITLE FROM DEPT;
```

```
 DEPT
------
 ENG
 HR
 OPS
 NULL
(4 rows)
 DEPT
------
 HR
(1 row)
 DEPT
------
 ENG
 HR
 OPS
 NULL
(4 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): character strings and numbers are brought together [ISO/IEC 9075:1992 §7.10 SR12b ii, §9.3 SR2, SR3a] (at character 21)
```

## Table value constructors

`VALUES (a, b), (c, d), ...` is a table of the rows written, its columns
of a type each row's values fit. It is a query of its own, and gives
INSERT its rows. `TABLE T` is the whole of T.

```sql
VALUES (1, 'one'), (2, 'two');
SELECT * FROM (VALUES (1, 'one'), (2, 'two')) AS T (N, W) WHERE N = 2;
TABLE DEPT;
```

```
     |
-----+-----
   1 | one
   2 | two
(2 rows)
   N | W
-----+-----
   2 | two
(1 row)
 DEPT | TITLE
------+----------------------
 ENG  | Engineering
 HR   | People
(2 rows)
```

A row of one value may be written without parentheses: `VALUES 1, 2` is
two rows.

The standard: ISO/IEC 9075:1992 §7.1–§7.11; X3.135-1986 §5.19–§5.25.
