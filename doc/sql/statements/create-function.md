# CREATE FUNCTION

[Statements](README.md)

## NAME

CREATE FUNCTION: define an SQL-invoked function

## SYNOPSIS

```
<SQL-invoked function> ::=
    CREATE FUNCTION <schema qualified routine name>
    ( [ <parameter name> <data type> [ , <parameter name> <data type> ]... ] )
    RETURNS <data type>
    [ <routine characteristic> ]...
    <SQL procedure statement>

<routine characteristic> ::=
      LANGUAGE SQL
    | SPECIFIC <specific name>
    | [ NOT ] DETERMINISTIC
    | CONTAINS SQL | READS SQL DATA | MODIFIES SQL DATA
    | RETURNS NULL ON NULL INPUT | CALLED ON NULL INPUT

<return statement> ::= RETURN { <value expression> | NULL }
```

FUNCTION, RETURNS, RETURN, SPECIFIC, DETERMINISTIC, CONTAINS, READS and
MODIFIES are read as words, not key words, so a table or column may
still be named by one.

## DESCRIPTION

CREATE FUNCTION defines a function of an SQL routine body, invoked by
its name in a value expression like a built-in function. Its body is a
single statement, which gives the function's value by a [RETURN](return.md)
statement. It may be a program too, a compound statement
([BEGIN](begin.md)) or another control statement, whose RETURN gives
the value, with variables, cursors, handlers, loops, and CALLs of
procedures.

An invocation evaluates each argument, in order, and stores its value
in its parameter, as an INSERT stores a value in a column: a value too
long for a parameter's type, or out of its range, fails the statement.
A function of RETURNS NULL ON NULL INPUT given a null returns null, its
body not run. CALLED ON NULL INPUT, the default, runs it. The value of
the body's RETURN is then stored in the result type in the same way.
A body that ends without executing a RETURN raises SQLSTATE 2F005.

A schema may hold several functions of one name, overloads, each of
other types of parameters or another number of them. An invocation
invokes the one its arguments' types choose. The candidates are those
the invoker holds EXECUTE on that take as many arguments as they have
parameters, each argument of a type that may be given to its
parameter. Then, argument by argument from the left, those whose
parameter's type comes first for the argument's type are kept, and one
must be left. CHARACTER comes before CHARACTER VARYING for a fixed-length
string, and for an exact number every exact type comes first, then the
approximate types, then DECFLOAT. A `?` argument chooses nothing, and
takes the type of the parameter of the function chosen. A function of
the schema that takes the arguments is invoked before a registered
function of the name.

In the body, a parameter is named by its name, or by the function's
name and its name. A column of a table the body reads hides a parameter
of its name.

The body is bound as the owner of the function's schema binds it, of
that schema: it uses the owner's privileges, whoever invokes it, and
sees the tables and functions there were when it was made. A view,
constraint, trigger, generated column or routine that invokes a
function keeps invoking the one it invoked when it was made, even once
an overload that would suit its arguments better is created. A function
defined in CREATE SCHEMA may be granted on in the same definition.

The SQL-data access, always written, says what the body may do:
CONTAINS SQL reads no table, and READS SQL DATA reads them. A function of
MODIFIES SQL DATA changes them too. It is invoked only where a value is
given to a statement, not read in a query: in a SET clause of UPDATE or
MERGE, a row of INSERT's or MERGE's VALUES, an argument of CALL, or a
value of a program. It runs as a statement nested in the one invoking
it, and its changes are undone with that statement. A query, a WHERE,
an ON, a CHECK, a trigger's WHEN, COALESCE and NULLIF invoke none. The
rows an UPDATE changes are found before it invokes one, so rows the
function inserts are not changed by it.
DETERMINISTIC says the function gives the same value for the same
arguments, as turborel takes on trust. NOT DETERMINISTIC is the default.
A CHECK constraint or a generated column may invoke only a function
that is deterministic and reads no table.

The owner holds EXECUTE on the function from the system, and may grant
it if each privilege the body needs is held grantable. A function has
a specific name, which names it apart from its name: SPECIFIC gives it,
else it is the function's name, an underscore and a number.

CREATE FUNCTION runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The function's schema exists and its user owns it. A function named
   without a schema is of the session's default schema. In CREATE
   SCHEMA, it is of the schema made.
2. Each other function of the name in the schema, a distinct type's
   among them, is told apart from it. Given arguments of the types of
   this function's parameters, this function alone is chosen. So
   F(INTEGER) beside F(BIGINT) or F(DECIMAL(5,2)) is refused, as every
   exact type comes first alike, while F(INTEGER) beside F(DOUBLE
   PRECISION), F(CHARACTER(3)) or F(INTEGER, INTEGER) is taken.
3. The specific name, if written, is of the function's schema, and no
   routine of the schema has it.
4. Each characteristic is written at most once, and the SQL-data access
   is written. The parameters are
   named, no two the same, and have no mode. No parameter or result is
   of a domain or a row type.
5. The body is no schema, transaction, connection, session or dynamic
   statement, and has no host or dynamic parameter. A body of CONTAINS
   SQL reads no table, and one of READS SQL DATA changes none, counting
   every statement a program holds.
6. The type of each RETURN value is one the result type may store.
7. An invocation of a function of MODIFIES SQL DATA is where the
   paragraph above lets it be.
8. LANGUAGE is SQL. Any other language, SQL SECURITY, a parameter's
   default, RETURNS TABLE, NO SQL and NEW SAVEPOINT LEVEL are not
   supported.
9. The transaction is not READ ONLY.

## PRIVILEGES

The schema is its user's. The owner holds the privileges the body needs,
and EXECUTE on each function it invokes.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -102 | The definition is not well formed |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or the owner lacks a privilege the body needs |
| 0A000 | -900 | A feature not supported is used |
| 25000 | -917 | The transaction is READ ONLY |

An invocation of a name no function of the schema or registered
function takes fails with SQLCODE -103, as does one two functions take
alike, and one only functions the invoker holds no EXECUTE on take
with -104. Once made, an invocation fails with the SQLSTATE of what
fails in it:
22001 or 22003 of a value an argument or the result cannot be stored
in, 2F005 of a body that ends without RETURN, or the SQLSTATE a SIGNAL
in it raises. GET DIAGNOSTICS shows the
function as ROUTINE_SCHEMA, ROUTINE_NAME and SPECIFIC_NAME, and of an
argument the parameter as PARAMETER_NAME, PARAMETER_MODE and
PARAMETER_ORDINAL_POSITION.

## EXAMPLES

A function of a value, and one that reads a table:

```sql
CREATE TABLE PRICES (ITEM INTEGER, AMOUNT DECIMAL(7,2));
INSERT INTO PRICES VALUES (1, 9.50), (2, 20.00);
CREATE FUNCTION TAXED (AMOUNT DECIMAL(7,2)) RETURNS DECIMAL(9,2)
  DETERMINISTIC CONTAINS SQL RETURNS NULL ON NULL INPUT
  RETURN AMOUNT * 1.20;
CREATE FUNCTION DEAREST () RETURNS DECIMAL(7,2) READS SQL DATA
  RETURN (SELECT MAX(AMOUNT) FROM PRICES);
SELECT ITEM, TAXED(AMOUNT) FROM PRICES ORDER BY ITEM;
SELECT DEAREST(), TAXED(CAST(NULL AS DECIMAL(7,2))) FROM (VALUES (0)) AS ONE (X);
```

```
OK
OK
OK
OK
         ITEM |
--------------+-------------
            1 |       11.40
            2 |       24.00
(2 rows)
           |
-----------+-------------
     20.00 |        NULL
(1 row)
```

Overloads of one name, each invoked by the types of its arguments:

```sql
CREATE FUNCTION DESCRIBED (N INTEGER) RETURNS VARCHAR(20) CONTAINS SQL
  RETURN 'exact';
CREATE FUNCTION DESCRIBED (N DOUBLE PRECISION) RETURNS VARCHAR(20) CONTAINS SQL
  RETURN 'approximate';
CREATE FUNCTION DESCRIBED (S VARCHAR(10)) RETURNS VARCHAR(20) CONTAINS SQL
  RETURN 'text';
SELECT DESCRIBED(1), DESCRIBED(2.5E0), DESCRIBED('a') FROM (VALUES (0)) AS ONE (X);
```

```
OK
OK
OK
                      |                      |
----------------------+----------------------+----------------------
 exact                | approximate          | text
(1 row)
```

A program for a body, which counts the prices under a bound with a
cursor, and a function of MODIFIES SQL DATA that logs each price it is
given as an UPDATE's SET clause invokes it:

```sql
CREATE FUNCTION UNDER (BOUND DECIMAL(7,2)) RETURNS INTEGER READS SQL DATA
BEGIN
  DECLARE N INTEGER DEFAULT 0;
  DECLARE A DECIMAL(7,2);
  DECLARE DONE BOOLEAN DEFAULT FALSE;
  DECLARE C CURSOR FOR SELECT AMOUNT FROM PRICES;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE;
  OPEN C;
  FETCH C INTO A;
  WHILE NOT DONE DO
    IF A < BOUND THEN SET N = N + 1; END IF;
    FETCH C INTO A;
  END WHILE;
  CLOSE C;
  RETURN N;
END;
SELECT UNDER(10.00), UNDER(100.00) FROM (VALUES (0)) AS ONE (X);
CREATE TABLE PRICE_LOG (ITEM INTEGER, OLD DECIMAL(7,2));
CREATE FUNCTION LOGGED (ITEM INTEGER, AMOUNT DECIMAL(7,2)) RETURNS DECIMAL(7,2) MODIFIES SQL DATA
BEGIN
  INSERT INTO PRICE_LOG VALUES (ITEM, AMOUNT);
  RETURN AMOUNT;
END;
UPDATE PRICES SET AMOUNT = LOGGED(ITEM, AMOUNT) + 1;
SELECT * FROM PRICE_LOG ORDER BY ITEM;
SELECT LOGGED(ITEM, AMOUNT) FROM PRICES;
```

```
OK
              |
--------------+--------------
            1 |            2
(1 row)
OK
OK
OK
         ITEM |       OLD
--------------+-----------
            1 |      9.50
            2 |     20.00
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): function ANN.LOGGED possibly modifies SQL-data, so it is not invoked here [ISO/IEC 9075-2:2023 §7.17 SR25, §11.49 SR11] (at character 7)
```

Rules refused, and an invocation that fails:

```sql
CREATE FUNCTION PEEK () RETURNS INTEGER CONTAINS SQL
  RETURN (SELECT COUNT(*) FROM PRICES);
CREATE FUNCTION TAXED (AMOUNT INTEGER) RETURNS INTEGER CONTAINS SQL RETURN AMOUNT;
CREATE FUNCTION TINY (N SMALLINT) RETURNS SMALLINT CONTAINS SQL RETURN N;
SELECT TINY(100000) FROM (VALUES (0)) AS ONE (X);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): the body of ANN.PEEK possibly reads SQL-data, and it is CONTAINS SQL (IA076) [ISO/IEC 9075-2:2023 §11.60 SR23d]
turborel: SQLCODE -103 (SQLSTATE 2A000): function ANN.TAXED (INTEGER) is not told apart from function ANN.TAXED_1 (DECIMAL(7,2)), as an invocation of arguments of its types would invoke either [ISO/IEC 9075-2:2023 §11.60 SR20b] (at character 16)
OK
turborel: SQLCODE -301 (SQLSTATE 22003): 100000 does not fit SMALLINT without losing leading digits [ISO/IEC 9075-2:2023 §9.18 GR4a ii; ISO/IEC 9075:1992 §9.2 GR3k]
```

## STANDARD

ISO/IEC 9075-2:2023 §11.60, §9.18, §10.4, §16.2, §4.35, Feature T321
(subfeatures 01, 03 and 05), T341, B033, B128, S203 and S204, and §9.6,
§9.7 and §11.60 SR20 of overloads. A body of a control statement is
ISO/IEC 9075-4:2006 draft §11.2, Feature P002, and where a function of
MODIFIES SQL DATA may be invoked §4.35.2, §7.17 SR25 and §6.12 SR1a.
SQL-86 and SQL-92 have no SQL-invoked routines.

## SEE ALSO

[CREATE PROCEDURE](create-procedure.md), [RETURN](return.md),
[DROP ROUTINE](drop-routine.md), [GRANT](grant.md),
[CREATE TYPE](create-type.md), [Information Schema](../information-schema.md),
[BEGIN](begin.md)
