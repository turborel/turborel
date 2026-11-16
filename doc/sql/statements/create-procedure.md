# CREATE PROCEDURE

[Statements](README.md)

## NAME

CREATE PROCEDURE: define an SQL-invoked procedure

## SYNOPSIS

```
<SQL-invoked procedure> ::=
    CREATE PROCEDURE <schema qualified routine name>
    ( [ <SQL parameter declaration> [ , <SQL parameter declaration> ]... ] )
    [ <routine characteristic> ]...
    <SQL procedure statement>

<SQL parameter declaration> ::= [ IN | OUT | INOUT ] <parameter name> <data type>

<routine characteristic> ::=
      LANGUAGE SQL
    | SPECIFIC <specific name>
    | [ NOT ] DETERMINISTIC
    | CONTAINS SQL | READS SQL DATA | MODIFIES SQL DATA
    | DYNAMIC RESULT SETS 0
    | OLD SAVEPOINT LEVEL
```

PROCEDURE is a key word. INOUT, SPECIFIC, DETERMINISTIC, CONTAINS,
READS and MODIFIES are read as words, not key words.

## DESCRIPTION

CREATE PROCEDURE defines a procedure of an SQL routine body, run by
[CALL](call.md). Its body is a single statement: an INSERT, UPDATE,
DELETE, MERGE, SELECT INTO, GET DIAGNOSTICS or a CALL of another
procedure.

A parameter's mode says how it passes its value. IN, the default,
takes its argument's value. OUT starts null, and its value at the end
is assigned to its argument, a target. INOUT does both. In the body, a
parameter is named by its name, or by the procedure's name and its
name, and SELECT INTO, GET DIAGNOSTICS and a nested CALL may assign one
of OUT or INOUT.

The body runs as the owner of the procedure's schema, of that schema:
CURRENT_USER in it is that owner, and it uses the owner's privileges,
whoever calls it. It is bound as it is defined, and sees the tables and
routines there were when it was made. It has a diagnostics area of its
own, so GET DIAGNOSTICS in it reads what its own statement did.

The SQL-data access, always written, says what the body may do:
CONTAINS SQL reads and changes no table, READS SQL DATA reads them, and
MODIFIES SQL DATA changes them too. A BEFORE trigger may call only a
procedure that does not change SQL-data.

The owner holds EXECUTE on the procedure from the system, grantable if
each privilege the body needs is held grantable. A procedure has a
specific name, given by SPECIFIC, else its name, an underscore and a
number.

CREATE PROCEDURE runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The procedure's schema exists and its user owns it. A procedure
   named without a schema is of the session's default schema. In CREATE
   SCHEMA, it is of the schema made.
2. The schema has no other routine of the name: overloading is of
   Feature T341, not supported (SQLCODE -900).
3. The specific name, if written, is of the procedure's schema, and no
   routine of the schema has it.
4. Each characteristic is written at most once, and the SQL-data access
   is written. The parameters are named, no two the same, and no
   parameter is of a domain or a row type. A procedure has no RETURNS
   and no null-call clause.
5. The body is no schema, transaction, connection, session or dynamic
   statement, has no host or dynamic parameter, and is no RETURN. A body
   of CONTAINS SQL reads no table. One of READS SQL DATA changes none.
6. LANGUAGE is SQL. Any other language, SQL SECURITY, a parameter's
   default, DYNAMIC RESULT SETS above 0 and NEW SAVEPOINT LEVEL are not
   supported.
7. The transaction is not READ ONLY.

## PRIVILEGES

The schema is its user's. The owner holds the privileges the body needs,
and EXECUTE on each routine it invokes.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -102 | The definition is not well formed |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or the owner lacks a privilege the body needs |
| 0A000 | -900 | Another routine of the name is in the schema, or a feature not supported is used |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

A procedure that records an order and one that reads a count back:

```sql
CREATE TABLE ORDERS (ID INTEGER, QUANTITY INTEGER);
CREATE PROCEDURE PLACE (IN ID INTEGER, IN QUANTITY INTEGER) MODIFIES SQL DATA
  INSERT INTO ORDERS VALUES (ID, QUANTITY);
CREATE PROCEDURE PLACED (OUT N INTEGER) READS SQL DATA
  SELECT COUNT(*) INTO N FROM ORDERS;
CALL PLACE(1, 5);
CALL PLACE(2, 7);
SELECT * FROM ORDERS ORDER BY ID;
```

```
OK
OK
OK
OK
OK
           ID |     QUANTITY
--------------+--------------
            1 |            5
            2 |            7
(2 rows)
```

Rules refused:

```sql
CREATE PROCEDURE SNEAKY (IN ID INTEGER) READS SQL DATA
  DELETE FROM ORDERS WHERE ORDERS.ID = ID;
CREATE PROCEDURE PLACE (IN ID INTEGER) MODIFIES SQL DATA
  DELETE FROM ORDERS;
CREATE PROCEDURE ENDED () MODIFIES SQL DATA COMMIT;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): the body of ANN.SNEAKY possibly modifies SQL-data, and it is READS SQL DATA (IA075) [ISO/IEC 9075-2:2023 §11.60 SR23c]
turborel: SQLCODE -900 (SQLSTATE 0A000): schema ANN has a routine PLACE already: overloading (Feature T341) is not supported [ISO/IEC 9075-2:2023 §11.60 CR2] (at character 17)
turborel: SQLCODE -103 (SQLSTATE 2A000): an SQL routine body is an INSERT, UPDATE, DELETE, MERGE, SELECT INTO, CALL, GET DIAGNOSTICS or a function's RETURN (IA078) [ISO/IEC 9075-2:2023 §11.60 SR23f, SR23g]
```

## STANDARD

ISO/IEC 9075-2:2023 §11.60, §9.18, §4.35, Feature T321 (subfeatures 02
and 04), B128. SQL-86 and SQL-92 have no SQL-invoked routines.

## SEE ALSO

[CALL](call.md), [CREATE FUNCTION](create-function.md),
[DROP ROUTINE](drop-routine.md), [GRANT](grant.md),
[CREATE TRIGGER](create-trigger.md), [GET DIAGNOSTICS](get-diagnostics.md)
