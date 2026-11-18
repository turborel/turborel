# CREATE MODULE

[Statements](README.md)

## NAME

CREATE MODULE: define an SQL-server module, a set of routines and
temporary tables of their own

## SYNOPSIS

```
<SQL-server module definition> ::=
    CREATE MODULE <SQL-server module name>
    [ NAMES ARE <character set name> ]
    [ SCHEMA <unqualified schema name> ]
    [ PATH <schema name> [ { , <schema name> }... ] ]
    [ <temporary table declaration>... ]
    <module routine> ; [ { <module routine> ; }... ]
    END MODULE

<module routine> ::=
    [ DECLARE ] { <SQL-invoked procedure> | <SQL-invoked function> }
```

A `<temporary table declaration>` is that of [DECLARE LOCAL TEMPORARY
TABLE](declare-local-temporary-table.md). An `<SQL-invoked procedure>`
or `<SQL-invoked function>` is what follows CREATE in [CREATE
PROCEDURE](create-procedure.md) or [CREATE FUNCTION](create-function.md).

## DESCRIPTION

CREATE MODULE defines a module of a schema: its routines, which are
invoked as the schema's routines are, by their names in the module's
schema, and temporary tables its routines alone see. The module is a
schema element. [DROP MODULE](drop-module.md) drops it whole, with its
routines and tables.

A module's routine is no schema-level routine: DROP ROUTINE does not
drop it, and no privilege is granted on it. EXECUTE on the module
invokes each of its routines. The owner of the module's schema holds
it, granted by `_SYSTEM`, with grant option once it holds with grant
option every privilege the routines need.

A temporary table the module declares is named MODULE.T within the
module. Each session has its own instance of it, as of a declared local
temporary table, and ON COMMIT DELETE ROWS, the default, empties it as
each transaction ends. Outside the module MODULE.T names the session's
own declared tables, and no view of the Information Schema shows the
module's.

SCHEMA names the schema a name written without one stands for within
the module: a table's, a type's, a sequence generator's. It is the
module's own schema if not written. PATH lists the schemas a routine
name written without a schema is looked for in, in order. A function
invoked is of the first schema of the path that has one the arguments
fit, and a procedure called the first of its name and number of
arguments. Without PATH, the path is the module's schema.

NAMES ARE names the module's character set, SQL_TEXT, the one there is.

CREATE MODULE runs within the session's transaction: ROLLBACK undoes
it. INFORMATION_SCHEMA.MODULES shows the module, and ROUTINES its
routines, with MODULE_CATALOG, MODULE_SCHEMA and MODULE_NAME the
module's name and SCHEMA_LEVEL_ROUTINE NO.

## RULES

1. The schema has no module of the name, and its user owns the schema.
   In a [CREATE SCHEMA](create-schema.md), the module is of the schema
   defined.
2. Each routine is of the module's schema, and the schema has no other
   routine of its specific name.
3. The module declares no table twice.
4. The path names each schema once, each of this catalog.
5. NAMES ARE names SQL_TEXT.
6. The transaction is not READ ONLY.

## PRIVILEGES

The schema is its user's. Its user holds the privileges each routine's
body needs, as for CREATE FUNCTION and CREATE PROCEDURE.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The text is not a module definition: no routine, no semicolon after one, no END MODULE |
| 2A000 | -103 | A rule above is broken, or a routine's body breaks a rule of CREATE FUNCTION or CREATE PROCEDURE |
| 2A000 | -104 | Its user does not own the schema, or holds not a privilege a routine's body needs |
| 0A000 | -900 | NAMES ARE names a character set other than SQL_TEXT |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE SCHEMA LIB CREATE FUNCTION TAX (AMOUNT DECIMAL(9,2)) RETURNS DECIMAL(9,2) CONTAINS SQL RETURN AMOUNT * 0.2;
CREATE TABLE ORDERS (ID INTEGER, AMOUNT DECIMAL(9,2));
CREATE MODULE SHOP PATH LIB, ANN
  DECLARE LOCAL TEMPORARY TABLE MODULE.PLACED (ID INTEGER) ON COMMIT PRESERVE ROWS
  PROCEDURE PLACE (IN ID INTEGER, IN AMOUNT DECIMAL(9,2)) MODIFIES SQL DATA
    BEGIN
      INSERT INTO ORDERS VALUES (ID, AMOUNT + TAX(AMOUNT));
      INSERT INTO MODULE.PLACED VALUES (ID);
    END;
  FUNCTION PLACED () RETURNS INTEGER READS SQL DATA
    RETURN (SELECT COUNT(*) FROM MODULE.PLACED);
END MODULE;
CALL PLACE(1, 10);
CALL PLACE(2, 20);
COMMIT;
SELECT ID, AMOUNT, PLACED() AS PLACED FROM ORDERS ORDER BY ID;
SELECT ID FROM MODULE.PLACED;
SELECT ROUTINE_NAME, MODULE_NAME, SCHEMA_LEVEL_ROUTINE FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'ANN';
SELECT MODULE_NAME, DEFAULT_SCHEMA_NAME, SQL_PATH FROM INFORMATION_SCHEMA.MODULES;
```

```
OK
OK
OK
OK
OK
OK
           ID |      AMOUNT |       PLACED
--------------+-------------+--------------
            1 |       12.00 |            2
            2 |       24.00 |            2
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): table MODULE.PLACED is not defined [X3.135-1986 §5.4 SR5] (at character 15)
 ROUTINE_NAME                                                 | MODULE_NAME                                                  | SCHEMA_LEVEL_ROUTINE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 PLACE                                                        | SHOP                                                         | NO
 PLACED                                                       | SHOP                                                         | NO
(2 rows)
 MODULE_NAME                                                  | DEFAULT_SCHEMA_NAME                                          | SQL_PATH
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 SHOP                                                         | ANN                                                          | PATH LIB, ANN
(1 row)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §9.21, §9.24, §12.8, Feature P001. SQL-86,
SQL-92 and ISO/IEC 9075-2 have no SQL-server modules.

## SEE ALSO

[DROP MODULE](drop-module.md), [CREATE PROCEDURE](create-procedure.md),
[CREATE FUNCTION](create-function.md), [GRANT](grant.md),
[Information Schema](../07-information-schema.md)
