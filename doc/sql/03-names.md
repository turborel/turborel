# Names and Qualification

[Contents](README.md)

## Schema-qualified names

A table, a view, a domain and a constraint are each of a schema, and
named in it: `ANN.STAFF` is the table STAFF of the schema ANN. Two
objects of one schema have different names; objects of different
schemas may have one name. A schema is named as its owner is, as a rule,
by `CREATE SCHEMA AUTHORIZATION ANN`, and may be given another name,
`CREATE SCHEMA SHOP`.

A name written without a schema is of the session's **default schema**:
the session's authorization identifier as it begins, then the one [SET
SCHEMA](statements/set-schema.md) sets.

```sql
SET SCHEMA 'SHOP';
SELECT COUNT(*) FROM ITEMS;
SELECT COUNT(*) FROM STAFF;
SELECT COUNT(*) FROM ANN.STAFF;
```

```
OK

----------------------
                    0
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): table SHOP.STAFF is not defined [X3.135-1986 §5.4 SR5] (at character 21)

----------------------
                    1
(1 row)
```

An index's name is of no schema: no two indexes of the database have
one name. A user's name is an authorization identifier.

## Catalog-qualified names

A database is one catalog, named by its setting CATALOG.NAME, TURBOREL
unless set. A schema's name may be qualified by it, and so an object's:
`TURBOREL.ANN.STAFF`. A catalog named that is not it is refused:

```sql
SELECT COUNT(*) FROM TURBOREL.ANN.STAFF;
SELECT COUNT(*) FROM OTHER.ANN.STAFF;
```

```

----------------------
                    1
(1 row)
turborel: SQLCODE -103 (SQLSTATE 3D000): the one catalog is TURBOREL, not OTHER [ISO/IEC 9075:1992 §5.4 SR20] (at character 21)
```

A definition kept by the database, a view's query or a constraint's
condition, keeps the catalog name it was written with: if CATALOG.NAME
changes, it reads as before.

## MODULE

MODULE qualifies the name of a table a session declares,
[DECLARE LOCAL TEMPORARY TABLE](statements/declare-local-temporary-table.md):
`MODULE.SCRATCH` is the session's own, in no schema. MODULE is a key
word, and no schema is named MODULE.

## How an unqualified name is resolved

**Objects.** A table, view or domain named without a schema is of the
session's default schema as the statement is read. In a [CREATE
SCHEMA](statements/create-schema.md), it is of the schema being
defined. A view's query, a CHECK's condition and a default are kept with
the schema their names were read in: a view defined in SHOP that reads
ITEMS reads SHOP.ITEMS from then on, whatever schema the session reading
it has.

A constraint's name written without a schema, a table's or a domain's,
is of the default schema too, which may be another than its table's or
domain's; one the implementation gives is of its table's or domain's
schema. Only the owner of a schema names a constraint in it.
A domain named without a schema, where the default schema has no domain
of the name, may name a type a plug-in registers.

**Columns.** A column is named as it is, or qualified by the table or
view it is of, `STAFF.NAME`, or by a correlation name the FROM clause
gives that table, `S.NAME`. Once a table is given a correlation name,
its own name no longer qualifies its columns in that query. A column
named without a qualifier is of the one table in the FROM clause that
has a column of the name; if two have, the name is ambiguous, and
refused.

In a subquery, a column is looked for first among the subquery's own
tables, then among those of the query around it, and so on outward: a
column of an outer query is an **outer reference**, its value the one
of the row the outer query is on.

```sql
SELECT NAME FROM STAFF, DEPT;
SELECT S.NAME, D.NAME FROM STAFF S, DEPT D WHERE S.DEPT = D.CODE;
SELECT NAME FROM DEPT WHERE EXISTS (SELECT * FROM STAFF WHERE DEPT = CODE);
SELECT STAFF.NAME FROM STAFF S;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): column NAME is ambiguous; qualify it [X3.135-1986 §5.7 SR3b] (at character 7)
 NAME                           | NAME
--------------------------------+--------------------------------
 Ada                            | Engineering
(1 row)
 NAME
--------------------------------
 Engineering
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): no table or correlation name STAFF is in scope [X3.135-1986 §5.7 SR3a] (at character 7)
```

In the third, DEPT is the column of STAFF, the subquery's own table, and
CODE of DEPT, the outer query's.

In a program, a name in a statement that is a parameter's is the
parameter: see [DECLARE CURSOR](statements/declare-cursor.md).

The standard: ISO/IEC 9075:1992 §5.4 and §6.4; X3.135-1986 §5.4 and
§5.7. That only a schema's owner names a constraint in it is stricter
than SQL-92.
