# GRANT

[Statements](README.md)

## NAME

GRANT: give privileges

## SYNOPSIS

```
<grant statement> ::=
    GRANT <privileges> ON <object name>
    TO <grantee> [ { , <grantee> }... ]
    [ WITH GRANT OPTION ]

<privileges> ::= ALL PRIVILEGES | <action> [ { , <action> }... ]

<action> ::=
      SELECT | DELETE
    | INSERT [ ( <column name list> ) ]
    | UPDATE [ ( <column name list> ) ]
    | REFERENCES [ ( <column name list> ) ]
    | USAGE
    | EXECUTE

<object name> ::=
      [ TABLE ] <table name>
    | DOMAIN <domain name>
    | COLLATION <collation name>
    | SEQUENCE <sequence generator name>
    | TYPE <user-defined type name>
    | SPECIFIC FUNCTION <specific name>
    | FUNCTION <function name> [ ( [ <data type> [ { , <data type> }... ] ] ) ]

<grantee> ::= PUBLIC | <authorization identifier>
```

## DESCRIPTION

GRANT gives each grantee privileges on a table, a view, a domain, a
collation CREATE COLLATION made, a sequence generator, a distinct type,
or a function a distinct type's definition made:

| Privilege | Lets its holder |
|---|---|
| SELECT | read the table |
| INSERT | insert rows; on columns, insert rows naming only those columns |
| UPDATE | update the table; on columns, set only those columns |
| DELETE | delete rows |
| REFERENCES | name the table, or the columns, in a constraint: a foreign key that references them, a CHECK or an assertion that reads them |
| USAGE | use the domain: define a column of it, CAST to it; use the collation: name it in COLLATE; use the sequence generator: take its values by NEXT VALUE FOR; use the distinct type: name it as a column's type or a CAST's |
| EXECUTE | invoke the function, by name or by a CAST to or from its distinct type |

A function is named by its specific name, the type's name and
_TO_DISTINCT or _TO_SOURCE, or by its name and, if more than one function
of the schema has it, its parameter's type.
INSERT, UPDATE or REFERENCES without columns is the privilege on the
table and on each of its columns, and on each column added to it later.
ALL PRIVILEGES is every privilege its user may grant on the object.
PUBLIC is every authorization identifier, present and to come.

The privileges granted are those its user holds with grant option: the
owner of the object's schema holds every privilege on a base table
with grant option, and those of a view as the tables below give them.
A privilege it does not hold with grant option is not granted, and the
statement completes with the warning SQLSTATE 01007. WITH GRANT OPTION
lets the grantees grant the privileges in turn.

GRANT runs within the session's transaction: ROLLBACK undoes it.
INFORMATION_SCHEMA.TABLE_PRIVILEGES, COLUMN_PRIVILEGES,
USAGE_PRIVILEGES, UDT_PRIVILEGES and ROUTINE_PRIVILEGES show the
privileges.

## RULES

1. The object exists. A table named without a schema is of the
   session's default schema.
2. Of a domain, a collation, a sequence generator or a type, USAGE alone
   is granted, and of a function EXECUTE alone; of a table, neither.
3. A function's name designates exactly one function.
4. Each column named is the table's.
5. Of a temporary table, ALL PRIVILEGES alone is granted.
6. The transaction is not READ ONLY.

## PRIVILEGES

Each privilege granted is held by its user with grant option, or it is
not granted (rule above, SQLSTATE 01007).

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The statement is not written as the synopsis shows: a column list of SELECT, among others |
| 2A000 | -103 | A rule above is broken |
| 0A000 | -900 | The object is a character set or a translation |
| 25000 | -917 | The transaction is READ ONLY |
| 01007 | 0 | Not an error: a privilege was not held with grant option, and is not granted |

## EXAMPLES

Of a table STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), SALARY
DECIMAL(9,2)), run by its owner ANN:

```sql
GRANT SELECT, UPDATE (SALARY) ON STAFF TO BOB WITH GRANT OPTION;
GRANT INSERT ON TABLE STAFF TO PUBLIC;
COMMIT;
```

Then by BOB:

```sql
SELECT NAME FROM ANN.STAFF;
UPDATE ANN.STAFF SET NAME = 'x';
UPDATE ANN.STAFF SET SALARY = 7300;
GRANT SELECT ON ANN.STAFF TO CAROL;
GRANT DELETE ON ANN.STAFF TO CAROL;
```

```
 NAME
--------------------------------
 Ada
(1 row)
turborel: SQLCODE -104 (SQLSTATE 2A000): BOB has no UPDATE(NAME) privilege on ANN.STAFF [X3.135-1986 §8.12 SR1, §6.6 SR3] (at character 7)
OK
OK
OK, with a warning: SQLSTATE 01007
```

BOB holds no DELETE to grant: CAROL is given none. The privileges, as
the Information Schema shows them, ANN's own left out:

```sql
SELECT GRANTOR, GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES
    WHERE TABLE_NAME = 'STAFF' AND GRANTEE <> 'ANN' ORDER BY 2, 3;
```

```
 GRANTOR                                                      | GRANTEE                                                      | PRIVILEGE_TYPE                                               | IS_GRANTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ANN                                                          | BOB                                                          | SELECT                                                       | YES
 ANN                                                          | PUBLIC                                                       | INSERT                                                       | NO
(2 rows)
```

BOB's UPDATE is on SALARY alone, so it is a column privilege, in
COLUMN_PRIVILEGES. Grants refused:

```sql
GRANT SELECT ON NOPE TO BOB;
GRANT USAGE ON STAFF TO BOB;
GRANT SELECT (NAME) ON STAFF TO BOB;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.NOPE is not defined [X3.135-1986 §5.4 SR5] (at character 16)
turborel: SQLCODE -103 (SQLSTATE 2A000): USAGE is granted on a domain, not on table ANN.STAFF [ISO/IEC 9075:1992 §11.36 SR1] (at character 15)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected ON, found "(" (at character 13)
```

## STANDARD

ISO/IEC 9075:1992 §11.36, and X3.135-1986 §6.6 (the `<privilege
definition>` of a `<schema>`), whose privileges are on tables, with
UPDATE alone of a column list. Outside a `<schema definition>`, GRANT is
SQL-92's `<SQL schema statement>` [§20.1]. Of a collation or a sequence
generator, ISO/IEC 9075-2:2023 §12.2 and §12.3.

## SEE ALSO

[REVOKE](revoke.md), [CREATE SCHEMA](create-schema.md),
[CREATE USER](create-user.md)
