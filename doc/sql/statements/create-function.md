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

<polymorphic table function> ::=
    CREATE FUNCTION <schema qualified routine name>
    ( [ <PTF parameter> [ , <PTF parameter> ]... ] )
    RETURNS { TABLE [ ( <column name> <data type> [ , ... ] ) ] | ONLY PASS THROUGH }
    [ <routine characteristic> ]...
    [ PRIVATE [ DATA ] ( [ <parameter name> <data type> [ DEFAULT <value expression> ] [ , ... ] ] ) ]
    [ DESCRIBE WITH <specific routine designator> ]
    [ START WITH <specific routine designator> ]
    FULFILL WITH <specific routine designator>
    [ FINISH WITH <specific routine designator> ]

<PTF parameter> ::=
      <parameter name> <data type>
    | <parameter name> TABLE [ [ NO ] PASS THROUGH ]
        [ WITH ROW SEMANTICS | WITH SET SEMANTICS [ PRUNE ON EMPTY | KEEP ON EMPTY ] ]
    | <parameter name> DESCRIPTOR
```

FUNCTION, RETURNS, RETURN, SPECIFIC, DETERMINISTIC, CONTAINS, READS,
MODIFIES, PASS, THROUGH, SEMANTICS, PRUNE, KEEP, EMPTY, PRIVATE, START,
FULFILL and FINISH are read as words, not key words, so a table or
column may still be named by one.

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

### Polymorphic table functions

A function of a generic table parameter, TABLE, or a descriptor
parameter, DESCRIPTOR, is a polymorphic table function. It is invoked
in FROM ([SELECT](select.md)), each table argument a table it reads, and
its result is a table whose columns may depend on its arguments. Its
body names procedures, its component procedures, which do the work.

A table parameter of row semantics is given its rows in no partition,
the whole table to one run of the components. One of set semantics, the default, is
given them partitioned by its argument's PARTITION BY, each partition
to one run of the components, in its ORDER BY's order. Of PRUNE ON
EMPTY, an empty table gives no row, as does an empty table of row
semantics. Of KEEP ON EMPTY, the default, an argument may say PRUNE or
KEEP WHEN EMPTY. A table of PASS THROUGH gives the result its columns,
of the row each result row names. Else the result has its partitioning
columns.

RETURNS TABLE with columns gives the proper result columns. Without
them, the describe component gives them as each invocation is bound.
RETURNS ONLY PASS THROUGH gives none, the result its tables' columns.

The describe component runs as an invocation is bound. It reads the
row type of each table argument and says the columns of it the other
components read. The start component runs first for each partition,
then the fulfill component, which reads the rows by FETCH and gives
result rows by [PIPE ROW](pipe-row.md), then the finish component.
Each is a procedure whose parameters follow a fixed list. The private
parameters first, INOUT, IN of finish. Then of each data parameter its
value, of a descriptor parameter its descriptor area's name. Of each
table parameter, describe takes its row type's and its requested row
type's names, fulfill its cursor row type's and its cursor's, each the
partitioning and ordering descriptors' if of set semantics. Then the
result row type's name, which describe takes only if RETURNS TABLE has
no columns. Last the status, INOUT CHARACTER(5). Each name is
VARCHAR(128). A component reads and sets these by
[GET DESCRIPTOR](get-descriptor.md), [SET DESCRIPTOR](set-descriptor.md),
[COPY DESCRIPTOR](copy-descriptor.md) and [FETCH](fetch.md) of PTF and
the name. A status it leaves other than `00000` raises that SQLSTATE.

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
5. The body is no schema, connection, session or dynamic statement, no
   COMMIT and no ROLLBACK but ROLLBACK TO SAVEPOINT, and has no host or
   dynamic parameter. A body of CONTAINS
   SQL reads no table, and one of READS SQL DATA changes none, counting
   every statement a program holds.
6. The type of each RETURN value is one the result type may store.
7. An invocation of a function of MODIFIES SQL DATA is where the
   paragraph above lets it be.
8. LANGUAGE is SQL. Any other language, SQL SECURITY, a parameter's
   default, RETURNS TABLE of a function of no table or descriptor
   parameter and NO SQL are not supported. A function has no savepoint
   level indication, as it always runs at a savepoint level of its own (see
   [SAVEPOINT](savepoint.md)).
9. The transaction is not READ ONLY.
10. Only a polymorphic table function has a table or descriptor
    parameter, and it has a body of component procedures. Its result
    columns are named apart, a private parameter's default stored in
    it.
11. Each component designated is one procedure, of the parameter list
    above, its modes and types alike, of no MODIFIES SQL DATA nor more
    SQL-data access than the function's. Describe is deterministic, and
    each is if the function is. A function of RETURNS TABLE of no
    columns has a describe component.

## PRIVILEGES

The schema is its user's. The owner holds the privileges the body needs,
and EXECUTE on each function it invokes. The owner of a polymorphic
table function holds EXECUTE on each of its components.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 42000 | -102 | The definition is not well formed |
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | Its user does not own the schema, or the owner lacks a privilege the body needs |
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

A `<PTF derived table>` that breaks a rule of its invocation, or whose
describe component raises an exception or leaves a status other than
`00000`, fails with SQLCODE -103 as it is bound. One whose describe
component reads SQL-data, in a view's definition, fails with -900. As
it runs, a component's exception, or the status it leaves, is the
query's SQLSTATE.

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
turborel: SQLCODE -103 (SQLSTATE 42000): function ANN.LOGGED possibly modifies SQL-data, so it is not invoked here [ISO/IEC 9075-2:2023 §7.17 SR25, §11.49 SR11] (at character 7)
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
turborel: SQLCODE -103 (SQLSTATE 42000): the body of ANN.PEEK possibly reads SQL-data, and it is CONTAINS SQL (IA076) [ISO/IEC 9075-2:2023 §11.60 SR23d]
turborel: SQLCODE -103 (SQLSTATE 42000): function ANN.TAXED (INTEGER) is not told apart from function ANN.TAXED_1 (DECIMAL(7,2)), as an invocation of arguments of its types would invoke either [ISO/IEC 9075-2:2023 §11.60 SR20b] (at character 16)
OK
turborel: SQLCODE -301 (SQLSTATE 22003): 100000 does not fit SMALLINT without losing leading digits [ISO/IEC 9075-2:2023 §9.18 GR4a ii; ISO/IEC 9075:1992 §9.2 GR3k]
```

A polymorphic table function, and the rules its definition and its
invocation keep. Its fulfill component gives each row of its table
doubled, the table's columns passed through ([PIPE ROW](pipe-row.md)
and [SELECT](select.md) show more):

```sql
CREATE TABLE T (K INTEGER, N INTEGER);
INSERT INTO T VALUES (1, 10), (2, 20);
CREATE PROCEDURE TWICE_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5))
  CONTAINS SQL
BEGIN
  DECLARE K, N, D INTEGER;
  DECLARE S BIGINT;
  DECLARE DONE BOOLEAN DEFAULT FALSE;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE;
  L: LOOP
    FETCH NEXT FROM PTF CU INTO K, N, S;
    IF DONE THEN LEAVE L; END IF;
    SET D = N * 2;
    SET DESCRIPTOR PTF RR VALUE 1 DATA = D;
    SET DESCRIPTOR PTF RR VALUE 2 DATA = S;
    PIPE ROW PTF RR;
  END LOOP L;
END;
CREATE FUNCTION TWICE (T TABLE PASS THROUGH WITH ROW SEMANTICS) RETURNS TABLE (D INTEGER)
  CONTAINS SQL FULFILL WITH PROCEDURE TWICE_F;
SELECT P.D, X.K, X.N FROM TABLE (TWICE (TABLE (T) AS X)) AS P ORDER BY X.K;
CREATE FUNCTION UNDESCRIBED (T TABLE PASS THROUGH WITH ROW SEMANTICS) RETURNS TABLE
  CONTAINS SQL FULFILL WITH PROCEDURE TWICE_F;
CREATE FUNCTION BY_SET (T TABLE PASS THROUGH) RETURNS TABLE (D INTEGER)
  CONTAINS SQL FULFILL WITH PROCEDURE TWICE_F;
SELECT * FROM TABLE (TWICE (TABLE (T)));
SELECT * FROM TABLE (TWICE (TABLE (T) PARTITION BY K)) AS P;
```

```
OK
OK
OK
OK
            D |            K |            N
--------------+--------------+--------------
           20 |            1 |           10
           40 |            2 |           20
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 42000): a polymorphic table function of no <table function column list> nor ONLY PASS THROUGH has a PTF describe component procedure [ISO/IEC 9075-2:2023 §11.60 SR10m]
turborel: SQLCODE -103 (SQLSTATE 42000): the parameters of PTF fulfill component procedure ANN.TWICE_F do not conform to (IN CHARACTER VARYING(128), IN CHARACTER VARYING(128), IN CHARACTER VARYING(128), IN CHARACTER VARYING(128), IN CHARACTER VARYING(128), INOUT CHARACTER(5)) [ISO/IEC 9075-2:2023 §11.60 SR10h]
turborel: SQLCODE -103 (SQLSTATE 42000): a <PTF derived table> has a correlation name [ISO/IEC 9075-2:2023 §7.6 SR7d ii] (at character 21)
turborel: SQLCODE -103 (SQLSTATE 42000): a table argument of a parameter of row semantics has no PARTITION BY [ISO/IEC 9075-2:2023 §9.18 SR12b iii 1] (at character 28)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.60, §9.18, §10.4, §16.2, §4.35, Feature T321
(subfeatures 01, 03 and 05), T341, B033, B128, S203 and S204. Polymorphic
table functions are §4.35.4, §9.24 to §9.27 and Features B200 to B209.
§9.6, §9.7 and §11.60 SR20 are of overloads. A body of a control statement is
ISO/IEC 9075-4:2006 draft §11.2, Feature P002, and where a function of
MODIFIES SQL DATA may be invoked §4.35.2, §7.17 SR25 and §6.12 SR1a.
SQL-86 and SQL-92 have no SQL-invoked routines.

## SEE ALSO

[CREATE PROCEDURE](create-procedure.md), [RETURN](return.md),
[DROP ROUTINE](drop-routine.md), [GRANT](grant.md),
[CREATE TYPE](create-type.md), [Information Schema](../07-information-schema.md),
[BEGIN](begin.md), [PIPE ROW](pipe-row.md), [COPY DESCRIPTOR](copy-descriptor.md),
[SELECT](select.md)
