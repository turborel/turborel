# Data Change Delta Tables

[Contents](README.md)

A data change delta table is the set of rows an INSERT, UPDATE, DELETE or
MERGE changed, read as a table by the query that runs the statement. It
gives back what the statement made, the identity values it generated,
the defaults and generated columns it filled in, and the rows it deleted. One
statement both changes the rows and reads them, so nothing else can come
between.

RETURNING *(extension)* writes the same query after the statement.

The examples on this page are run in order, by the user ANN:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE ORDERS (
    ID INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ITEM VARCHAR(20),
    QTY INTEGER DEFAULT 1,
    PRICE DECIMAL(7,2),
    TOTAL DECIMAL(9,2) GENERATED ALWAYS AS (QTY * PRICE));
COMMIT;
```

```
OK
OK
OK
```

## The delta table

```
<data change delta table> ::=
    <result option> TABLE <left paren> <data change statement> <right paren>
        [ [ AS ] <correlation name> [ <left paren> <derived column list> <right paren> ] ]

<result option> ::=
    OLD
  | NEW
  | FINAL

<data change statement> ::=
    <insert statement>
  | <update statement: searched>
  | <delete statement: searched>
  | <merge statement>
```

A delta table stands in the FROM clause of a query as its one table
reference. Its columns are the subject table's, by their names, unless a
derived column list renames them.

| Result option | Of an INSERT | Of an UPDATE | Of a DELETE | Of a MERGE |
|---|---|---|---|---|
| OLD | refused | each row updated, as it was | each row deleted | each row updated or deleted, as it was |
| NEW | each row inserted | each row updated, as it is now | refused | each row inserted or updated |
| FINAL | as NEW | as NEW | refused | as NEW |

A row is recorded as the statement writes it, after its BEFORE triggers,
its defaults, identity and generated columns given, and before its
referential actions and AFTER triggers run. So NEW gives no row an AFTER
trigger inserts, nor a change an AFTER trigger makes.

FINAL gives what NEW gives, and also makes sure those rows are final. No
referential action of the statement may change the subject table, nor
may an AFTER trigger. Either is refused, and the statement undone.

```sql
SELECT ID, TOTAL FROM NEW TABLE (
    INSERT INTO ORDERS (ITEM, PRICE) VALUES ('pen', 2.50), ('pad', 4.00)) ORDER BY ID;
SELECT ITEM, QTY FROM OLD TABLE (UPDATE ORDERS SET QTY = 3 WHERE ITEM = 'pen');
SELECT O.ITEM, O.T FROM FINAL TABLE (UPDATE ORDERS SET QTY = QTY + 1) AS O (I, ITEM, Q, P, T) ORDER BY O.I;
```

```
           ID |       TOTAL
--------------+-------------
            1 |        2.50
            2 |        4.00
(2 rows)
 ITEM                 |          QTY
----------------------+--------------
 pen                  |            1
(1 row)
 ITEM                 |           T
----------------------+-------------
 pen                  |       10.00
 pad                  |        8.00
(2 rows)
```

The statement runs first, in full, its constraints checked and its
triggers run. Then the query reads its rows. A statement that fails
gives no rows. The query fails with it, and the statement is undone. The
query's rows are all read as its cursor opens, so a query that fails
undoes the statement too.

A delta table is read by a query a cursor opens, directly, prepared or
declared, and by a SELECT INTO, in a routine too. Each opening of the
cursor runs the statement again, so a statement prepared changes rows at
each execution. Elsewhere a delta table is refused, in a subquery, a
join, a view or a WITH clause.

## RETURNING *(extension)*

```
<returning statement> ::=
    <data change statement> RETURNING <select list>
```

RETURNING makes a data change statement a query of the rows it changed:
of each row inserted or updated, the row as the statement left it, and
of each row deleted, the row as it was. The select list reads the
subject table's columns by their names. It is the query

```
SELECT <select list> FROM <delta table of the rows changed>
```

so it is run, prepared and read as a query, its rows a cursor's, over
the client's connection as any query's rows are.

```sql
INSERT INTO ORDERS (ITEM, PRICE) VALUES ('ink', 9.99) RETURNING ID, TOTAL;
DELETE FROM ORDERS WHERE QTY > 2 RETURNING ITEM;
UPDATE ORDERS SET PRICE = PRICE * 2 RETURNING *;
COMMIT;
```

```
           ID |       TOTAL
--------------+-------------
            3 |        9.99
(1 row)
 ITEM
----------------------
 pen
(1 row)
           ID | ITEM                 |          QTY |     PRICE |       TOTAL
--------------+----------------------+--------------+-----------+-------------
            2 | pad                  |            2 |      8.00 |       16.00
            3 | ink                  |            1 |     19.98 |       19.98
(2 rows)
OK
```

## Rules

1. OLD is refused of an INSERT, and of a MERGE that neither updates nor
   deletes. NEW and FINAL are refused of a DELETE, and of a MERGE that
   neither updates nor inserts.
2. The statement is not FOR PORTION OF, nor positioned.
3. The subject table is a base table.
4. The statement is one the transaction may run, so not in a READ ONLY
   transaction.

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The result option names rows the statement makes none of; the statement is FOR PORTION OF |
| 0A000 | -900 | The delta table is not the one table reference of a cursor's query or a SELECT INTO; its subject table is a view |
| 27001 | -911 | Under FINAL, a referential action of the statement changes the subject table |
| 09000 | | Under FINAL, an AFTER trigger changes the subject table: the triggered action exception, of SQLSTATE 0W001 |

Any error of the statement is the query's.

## Standard

ISO/IEC 9075-2:2023 §7.6, the `<data change delta table>`, Feature T495,
with §14.9, §14.12, §14.14, §15.11 and §15.14 for the old and new delta
tables. That a delta table is read only where a cursor's query or a
SELECT INTO has it as its one table reference is this implementation's
own (RULES.md D183). RETURNING is an extension (RULES.md D184).

## See also

[INSERT](statements/insert.md), [UPDATE](statements/update.md),
[DELETE](statements/delete.md), [MERGE](statements/merge.md),
[CREATE TRIGGER](statements/create-trigger.md),
[Query Expressions](06-queries.md)
