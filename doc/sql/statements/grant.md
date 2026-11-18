# GRANT

[Statements](README.md)

## NAME

GRANT: give privileges, or roles

## SYNOPSIS

```
<grant statement> ::= <grant privilege statement> | <grant role statement>

<grant privilege statement> ::=
    GRANT <privileges> ON <object name>
    TO <grantee> [ { , <grantee> }... ]
    [ WITH HIERARCHY OPTION ]
    [ WITH GRANT OPTION ]
    [ GRANTED BY <grantor> ]

<grant role statement> ::=
    GRANT <role name> [ { , <role name> }... ]
    TO <grantee> [ { , <grantee> }... ]
    [ WITH ADMIN OPTION ]
    [ GRANTED BY <grantor> ]

<grantor> ::= CURRENT_USER | CURRENT_ROLE

<privileges> ::= ALL PRIVILEGES | <action> [ { , <action> }... ]

<action> ::=
      SELECT | DELETE
    | INSERT [ ( <column name list> ) ]
    | UPDATE [ ( <column name list> ) ]
    | REFERENCES [ ( <column name list> ) ]
    | SELECT ( <method name> [ ( [ <data type> [ { , <data type> }... ] ] ) ] [ { , ... }... ] )
    | TRIGGER
    | USAGE
    | UNDER
    | EXECUTE

<object name> ::=
      [ TABLE ] <table name>
    | DOMAIN <domain name>
    | COLLATION <collation name>
    | SEQUENCE <sequence generator name>
    | TYPE <user-defined type name>
    | SPECIFIC METHOD <specific method name>
    | [ INSTANCE | STATIC | CONSTRUCTOR ] METHOD <method name> ( [ <data type> [ { , <data type> }... ] ] ) FOR <user-defined type name>
    | MODULE <SQL-server module name>
    | SPECIFIC <routine type> <specific name>
    | <routine type> <routine name> [ ( [ <data type> [ { , <data type> }... ] ] ) ]
    | XML SCHEMA <registered XML Schema name>

<routine type> ::= ROUTINE | FUNCTION | PROCEDURE

<grantee> ::= PUBLIC | <authorization identifier>
```

## DESCRIPTION

GRANT gives each grantee privileges on a table, a view, a domain, a
collation CREATE COLLATION made, a sequence generator, a user-defined
type, a function a distinct type's definition made, a method, a function or procedure
CREATE FUNCTION or CREATE PROCEDURE made, a module CREATE MODULE
made, or a registered XML Schema:

| Privilege | Lets its holder |
|---|---|
| SELECT | read the table |
| INSERT | insert rows; on columns, insert rows naming only those columns |
| UPDATE | update the table; on columns, set only those columns |
| DELETE | delete rows |
| REFERENCES | name the table, or the columns, in a constraint: a foreign key that references them, a CHECK or an assertion that reads them |
| SELECT (methods) | invoke those methods of the typed table's type through a reference whose scope is the table, `r -> m()` |
| UNDER | define a subtype of the type, or a subtable or subview of the typed table or view |
| TRIGGER | define a trigger on the table, in a schema of its own (see [CREATE TRIGGER](create-trigger.md)) |
| USAGE | use the domain: define a column of it, CAST to it; use the collation: name it in COLLATE; use the sequence generator: take its values by NEXT VALUE FOR; use the distinct type: name it as a column's type or a CAST's; use the registered XML Schema: name it in a type of XMLSCHEMA, XMLVALIDATE or IS VALID, validate by it without naming it, or import it in CREATE XML SCHEMA |
| EXECUTE | invoke the function, by name or by a CAST to or from its distinct type; run the procedure by CALL; invoke the method; invoke each routine of the module |

SELECT WITH HIERARCHY OPTION on a typed table is SELECT on each of its
subtables too, present and to come, and lets its holder name the table
with ONLY, as `FROM ONLY (t)`. REVOKE HIERARCHY OPTION FOR SELECT takes
the option back and leaves SELECT. SELECT on a typed table is SELECT on
each method of its type whose EXECUTE its grantor holds with grant
option.

A routine is named by its specific name, or by its name and, if more
than one routine of the schema has it, its parameters' types. A
distinct type's functions have the specific names of the type's name
and _TO_DISTINCT or _TO_SOURCE. ROUTINE names a function or a
procedure, FUNCTION only a function and PROCEDURE only a procedure.
EXECUTE on an attribute's observer lets its holder read the attribute,
and on its mutator set it. Their specific names are the type's name,
the attribute's and _OBSERVER or _MUTATOR, as POINT_X_OBSERVER, or they
are named as
`METHOD x () FOR t` and `METHOD x (type) FOR t`, of the type that
defines the attribute. A subtype's inherited attribute is read and set
by the same grants.
INSERT, UPDATE or REFERENCES without columns is the privilege on the
table and on each of its columns, and on each column added to it later.
ALL PRIVILEGES is every privilege its user may grant on the object.
PUBLIC is every authorization identifier, present and to come.

The privileges granted are those its user holds with grant option: the
owner of the object's schema holds every privilege on a base table
with grant option, EXECUTE on a routine or a module of it with grant
option if the routine's body, or each of the module's, needs only
privileges it holds with grant option, USAGE on a registered XML
Schema of it with grant option, and those of a view as the
tables below give them,
with TRIGGER on its own views too, but without grant option. An INSTEAD
OF trigger gives the owner of its schema the privilege of its event on
its view, with grant option.
Every user holds USAGE on the permanently registered XML Schemas,
INFORMATION_SCHEMA.XS, XSI and SQLXML, without grant option.
A privilege it does not hold with grant option is not granted, and the
statement completes with the warning SQLSTATE 01007. WITH GRANT OPTION
lets the grantees grant the privileges in turn.

The grantor is the session's user, or, with GRANTED BY CURRENT_ROLE,
the role it has set (see [SET ROLE](set-role.md)); without a user, as
in a trigger of a schema a role owns, the role. The privileges a grantor
holds are its own, and, of a role, those of each role applicable for it.
A privilege a role granted stands while the role holds it with grant
option, through its roles too.

GRANT of roles grants each role to each grantee, a user, a role or
PUBLIC. The grantor holds each role WITH ADMIN OPTION: granted so to it,
or to a role applicable for it. WITH ADMIN OPTION lets the grantees grant
the role, revoke it and drop it in turn. A role granted to a user brings
its privileges once the user's session sets it; a role granted to a role
is applicable for that role, so its privileges are the role's too.

GRANT runs within the session's transaction: ROLLBACK undoes it.
INFORMATION_SCHEMA.TABLE_PRIVILEGES, COLUMN_PRIVILEGES,
USAGE_PRIVILEGES, UDT_PRIVILEGES, ROUTINE_PRIVILEGES and
MODULE_PRIVILEGES show the privileges.

## RULES

1. The object exists. A table named without a schema is of the
   session's default schema.
2. Of a domain, a collation, a sequence generator, a type or a
   registered XML Schema, USAGE alone is granted, and of a routine or a
   module EXECUTE alone. Neither is granted of a table. No privilege is granted of a module's routine.
3. A routine's name designates exactly one routine of its routine type,
   counting the functions of distinct types. A name alone of overloads
   designates none, so one is named by its parameters' types or its
   specific name.
4. Each column named is the table's.
5. Of a temporary table, ALL PRIVILEGES alone is granted.
6. Of GRANTED BY CURRENT_USER, the session has a current user; of
   GRANTED BY CURRENT_ROLE, it has set a role.
7. Each role granted exists, and no grantee is the role or a role
   applicable for it: no role is granted to itself, in a cycle.
8. The transaction is not READ ONLY.

## PRIVILEGES

Each privilege granted is held by its grantor with grant option, or it
is not granted (SQLSTATE 01007). Each role granted is held by its
grantor WITH ADMIN OPTION (SQLSTATE 0P000).

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The statement is not written as the synopsis shows: a column list of SELECT, among others |
| 2A000 | -103 | A rule above is broken |
| 0A000 | -900 | The object is a character set or a translation |
| 0P000 | -104 | The grantor holds a role granted not WITH ADMIN OPTION |
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

EXECUTE on the observer and the mutator of an attribute of a type
POINT, by ANN, the type's owner. Y has both, so METHOD Y names neither
without its types:

```sql
CREATE TYPE POINT AS (X INTEGER, Y INTEGER) NOT FINAL;
GRANT EXECUTE ON SPECIFIC FUNCTION POINT_X_OBSERVER TO BOB;
GRANT EXECUTE ON METHOD X (INTEGER) FOR POINT TO BOB;
GRANT EXECUTE ON METHOD Y FOR POINT TO BOB;
COMMIT;
```

```
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): METHOD Y FOR ANN.POINT designates both the observer and the mutator of its attribute [ISO/IEC 9075-2:2023 §10.6 SR3c] (at character 24)
OK
```

Roles, by ANN, who may create them:

```sql
CREATE ROLE CLERK;
CREATE ROLE SENIOR;
GRANT CLERK TO SENIOR;
GRANT SENIOR TO BOB WITH ADMIN OPTION;
GRANT SENIOR TO CLERK;
COMMIT;
```

```
OK
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): CLERK is SENIOR or a role applicable for it, a cycle [ISO/IEC 9075-2:2023 §12.5 SR1] (at character 6)
OK
```

Then by BOB, who holds SENIOR WITH ADMIN OPTION but not CLERK, though
CLERK is applicable for SENIOR:

```sql
GRANT SENIOR TO CAROL;
GRANT CLERK TO CAROL;
COMMIT;
```

```
OK
turborel: SQLCODE -104 (SQLSTATE 0P000): BOB holds CLERK WITH ADMIN OPTION by none of its applicable roles [ISO/IEC 9075-2:2023 §12.5 GR1] (at character 6)
OK
```

## STANDARD

ISO/IEC 9075:1992 §11.36, and X3.135-1986 §6.6 (the `<privilege
definition>` of a `<schema>`), whose privileges are on tables, with
UPDATE alone of a column list. Outside a `<schema definition>`, GRANT is
SQL-92's `<SQL schema statement>` [§20.1]. Of a collation or a sequence
generator, ISO/IEC 9075-2:2023 §12.2 and §12.3. Of a registered XML
Schema, ISO/IEC 9075-14 leaves how USAGE is granted to the
implementation [§4.7.1], and XML SCHEMA is turborel's, granted as on a
sequence generator. GRANT of roles is
ISO/IEC 9075-2:2023 §12.5 (Feature T331), and GRANTED BY §12.8 (T332).

## SEE ALSO

[REVOKE](revoke.md), [CREATE SCHEMA](create-schema.md),
[CREATE USER](create-user.md), [CREATE ROLE](create-role.md),
[SET ROLE](set-role.md)
