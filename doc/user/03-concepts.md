# Concepts

[Contents](README.md)

## Catalogs and schemas

A database is one **catalog**, of **schemas**. A schema is a named set
of tables, views, domains and assertions, of one owner; each user's own
is named as the user, by `CREATE SCHEMA AUTHORIZATION ANN`, and may
have others. An object is named in its schema, `ANN.EMPLOYEES`; a name
written without one is of the session's default schema, the user's own
unless [SET SCHEMA](../sql/statements/set-schema.md) sets another
([Names](../sql/03-names.md)).

## Tables and views

A **base table** holds rows, each of a value for every column, of the
column's type ([Data Types](../sql/02-data-types.md)). A **view** is a
query given a name: it holds no rows, but gives those of its query each
time it is read.

```sql
CREATE VIEW ENGINEERS AS SELECT ID, NAME FROM EMPLOYEES WHERE DEPT = 'ENG';
SELECT * FROM ENGINEERS ORDER BY ID;
```

```
OK
           ID | NAME
--------------+--------------------------------
            1 | Ada
            2 | Grace
(2 rows)
```

A view of one table's rows and columns, as this one is, may be changed
through: an UPDATE of ENGINEERS changes EMPLOYEES
([CREATE VIEW](../sql/statements/create-view.md)).

## Domains

A **domain** is a data type with a default and constraints, which many
columns may share: a column of the domain MONEY is of its type, takes
its default, and keeps its constraints
([CREATE DOMAIN](../sql/statements/create-domain.md)).

## Users and authorization identifiers

Every session acts as an **authorization identifier**: a server's
client as the user it connected as. A schema is of one, and privileges
are granted to them. One user is the database's owner, who manages the
users ([Users](../operator/05-users.md)).

## Privileges

The owner of a schema may do anything to what it holds; anyone else only
what it is granted: SELECT to read a table, INSERT, UPDATE and DELETE to
change it, REFERENCES to name it in a constraint, USAGE to use a domain.
A privilege granted WITH GRANT OPTION may be granted on, and REVOKE takes
it back, with what it allowed ([GRANT](../sql/statements/grant.md),
[REVOKE](../sql/statements/revoke.md)).

```sql
GRANT SELECT ON ENGINEERS TO BOB;
```

```
OK
```

BOB may now read ENGINEERS, and so the engineers' names, but not
EMPLOYEES itself, nor anyone's salary.
