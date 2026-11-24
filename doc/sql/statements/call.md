# CALL

[Statements](README.md)

## NAME

CALL: run an SQL-invoked procedure

## SYNOPSIS

```
<call statement> ::= CALL <routine name> ( [ <SQL argument> [ , <SQL argument> ]... ] )
```

CALL is read as a word, not a key word.

## DESCRIPTION

CALL runs a procedure defined by [CREATE PROCEDURE](create-procedure.md)
with an argument for each of its parameters. A procedure named without
a schema is of the session's default schema. An argument may be named,
`K => 1`, and then every one is.

An argument of an IN parameter is a value expression, whose value is
stored in the parameter's type, or NULL, the null value. DEFAULT as an
argument of an IN or INOUT parameter gives it the default value its
procedure declares, or the null value if it declares none. A parameter
past the arguments written, or one no named argument names, takes its
default value so, and must have one. An INOUT parameter given DEFAULT
has no target, so its value out goes nowhere. A default is evaluated
with the other arguments, as the owner of the procedure's schema, so a
caller with EXECUTE alone may take a default that reads a table. An argument of an OUT or INOUT parameter
is a target: a host or embedded variable, a parameter of the procedure
whose body the CALL is, an SQL variable of a compound statement the
CALL is in, or, in a BEFORE row trigger, a column of its new transition
variable. Of the last two it may be one element of the
array, `P[i]` or `N.C[i]`, as [CREATE PROCEDURE](create-procedure.md)
says. Of a CALL prepared, the target may be a `?`, a dynamic parameter
of the parameter's type, an output, and of an INOUT parameter an input
too. An OUT parameter starts null. An INOUT
parameter starts with its target's value. Once the body has run, the
value of each OUT and INOUT parameter is assigned to its target, as
SELECT INTO assigns a value.

A CALL prepared with a `?` for an OUT or INOUT parameter is described
with a column for each such `?`, named and typed as its parameter, in
the parameters' order, as DESCRIBE OUTPUT shows; its input parameters
are the other `?`s, an INOUT parameter's among them. Executed, it gives
back the values of those columns. EXECUTE puts them where its INTO or
USING SQL DESCRIPTOR says. Over [the native protocol](../../client/05-protocol.md)
they come as a row before the statement's completion, which the client
library's `execute` gives as rows. The command-line client prepares a
CALL that has a `?`, and shows its outputs as a row.

Each `?` of a prepared CALL is described with the mode and the ordinal
position of the parameter whose argument holds it, and with the
procedure's specific name. DESCRIBE gives them as PARAMETER_MODE,
PARAMETER_ORDINAL_POSITION and the PARAMETER_SPECIFIC fields, the
embedded library's and the client library's `parameters` as each
`DynamicParameter`'s `mode` and `ordinal`, and `procedure` the specific
name. Several `?` marks of one IN argument, as `CALL P(? + ?)`, each have
that parameter's mode and position. A parameter left to its default
holds no `?`, and the others keep their own positions.

The body runs as the owner of the procedure's schema, of that schema,
with a diagnostics area of its own, within the CALL statement: all it
does shares the CALL's instant, and is undone if any part of it fails.
A procedure may call another, and a triggered action may call one.
Triggered actions and procedures nest at most 16 deep. An argument may
invoke a function that changes SQL-data, which runs before the
procedure, its changes the CALL's. A function's body may call a
procedure that changes none, which runs where the function does.

As a session first calls a procedure, its body is compiled to a program
the session keeps, and later calls run that program. A query of the
body that reads one row by a whole unique key reads it straight from
its table. A searched UPDATE or DELETE, alone or under FINAL, NEW or
OLD TABLE, and an INSERT of VALUES or of a query's rows run by the
plans kept of them, without being bound again. Such a change of one row
by its key writes that row straight to its table, and such an INSERT
writes its rows straight, together where there are several. Each checks
the constraints of the rows it wrote alone, unless a constraint it may
break is deferrable or reads other rows. Every other statement runs as
a statement the session runs. The program is compiled again once a
definition, privilege, role, schema, time zone or setting it was
compiled with changes. Its results,
errors, warnings and diagnostics are the same as of the body run
statement by statement, which the setting
[EXECUTOR.COMPILED_PROCEDURES](../../appendix/c-settings.md) OFF asks for.

GET DIAGNOSTICS after a CALL shows CALL as its COMMAND_FUNCTION, and of
a failure the procedure as ROUTINE_SCHEMA, ROUTINE_NAME and
SPECIFIC_NAME, and of an argument stored in a parameter the parameter
as PARAMETER_NAME, PARAMETER_MODE and PARAMETER_ORDINAL_POSITION.

## RULES

1. A procedure of the name takes the arguments: it has at least as
   many parameters, each one past those written, or named by none, has
   a default value, and each name names one of its parameters. Of those
   that take them, of the schema first in the path, the one of fewest
   parameters is invoked. Procedures of one name each have another
   number of parameters, so it is one alone. A `?` argument of an IN
   parameter takes the parameter's type.
2. Each argument is as its parameter's mode allows, and its type is one
   its parameter's or its target's takes. An OUT parameter takes a
   target alone, an INOUT one a target or DEFAULT.
3. A BEFORE trigger's action calls no procedure that possibly changes
   SQL-data, nor does a function's body that changes none.

## PRIVILEGES

EXECUTE on the procedure.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | The statement is not well formed |
| 42000 | -103 | No procedure takes the arguments, or a rule above is broken |
| 42000 | -104 | Its user holds no EXECUTE on the procedure |
| 22001 | -301 | A value is too long for a parameter or a target |
| 22003 | -301 | A value is out of the range of a parameter or a target |
| 0A000 | -900 | The nesting limit is passed |
| 07007 | -921 | EXECUTE of a CALL whose `?`s take outputs has no INTO |

A statement of the body that fails fails the CALL with its SQLSTATE.

## EXAMPLES

A procedure that changes a table, and one whose OUT parameter a BEFORE
trigger's CALL assigns to a column of the new row:

```sql
CREATE TABLE STOCK (ITEM INTEGER, QUANTITY INTEGER);
INSERT INTO STOCK VALUES (1, 10), (2, 4);
CREATE PROCEDURE TAKE (IN WHICH INTEGER, IN N INTEGER) MODIFIES SQL DATA
  UPDATE STOCK SET QUANTITY = QUANTITY - N WHERE ITEM = WHICH;
CREATE PROCEDURE LEFT_OF (IN WHICH INTEGER, OUT N INTEGER) READS SQL DATA
  SELECT QUANTITY INTO N FROM STOCK WHERE ITEM = WHICH;
CREATE TABLE ASKED (ITEM INTEGER, LEFT_THEN INTEGER);
CREATE TRIGGER LOOKED BEFORE INSERT ON ASKED REFERENCING NEW AS A
  FOR EACH ROW CALL LEFT_OF(A.ITEM, A.LEFT_THEN);
CALL TAKE(1, 3);
INSERT INTO ASKED (ITEM) VALUES (1), (2);
SELECT * FROM ASKED ORDER BY ITEM;
```

```
OK
OK
OK
OK
OK
OK
OK
OK
         ITEM |    LEFT_THEN
--------------+--------------
            1 |            7
            2 |            4
(2 rows)
```

A procedure of a default value, taken by DEFAULT, by an argument left
out and by a name not named:

```sql
CREATE PROCEDURE RESTOCK (IN WHICH INTEGER, IN N INTEGER DEFAULT 5) MODIFIES SQL DATA
  UPDATE STOCK SET QUANTITY = QUANTITY + N WHERE ITEM = WHICH;
CALL RESTOCK(1, DEFAULT);
CALL RESTOCK(2);
CALL RESTOCK(WHICH => 2);
SELECT * FROM STOCK ORDER BY ITEM;
```

```
OK
OK
OK
OK
         ITEM |     QUANTITY
--------------+--------------
            1 |           12
            2 |           14
(2 rows)
```

An argument refused, an argument left out of no default, and one too
large for its parameter:

```sql
CALL LEFT_OF(1, 5);
CALL TAKE(1);
CALL TAKE(99999999999, 1);
```

```
turborel: SQLCODE -103 (SQLSTATE 42000): the argument of parameter N of procedure ANN.LEFT_OF shall be a target specification [ISO/IEC 9075-2:2023 §9.18 SR9h iii 1]
turborel: SQLCODE -103 (SQLSTATE 42000): no procedure ANN.TAKE takes 1 arguments, each parameter past them of a default value; those of the name have 2 parameters [ISO/IEC 9075-2:2023 §9.18 SR9b]
turborel: SQLCODE -301 (SQLSTATE 22003): 99999999999 does not fit INTEGER without losing leading digits [ISO/IEC 9075-2:2023 §9.18 GR4c i; ISO/IEC 9075:1992 §9.2 GR3k]
```

A CALL prepared, a `?` for its OUT parameter, shown as a row:

```sql
CALL LEFT_OF(2, ?);
```

```
            N
--------------
            4
(1 row)
```

## STANDARD

ISO/IEC 9075-2:2023 §16.1, §9.18, §10.4, Feature T321-04. NULL and
DEFAULT as arguments, and arguments left out to their defaults, are of
Features T522 and T523 [§10.4 CR6, CR7, §9.18 SR9h i]; named arguments
of Feature T521. A `?` of an
OUT or INOUT parameter is an output dynamic parameter [§20.7 GR7a,
SR5a], described [§20.10], each `?` with its parameter's mode and
position [§20.10 GR7e], and put where EXECUTE's `<result using
clause>` says [§20.13 GR6, GR10]. An argument
`P[i]` is a `<target array element specification>` [§6.4], Feature
S097. SQL-86 and SQL-92 have no CALL.

## SEE ALSO

[CREATE PROCEDURE](create-procedure.md), [GRANT](grant.md),
[GET DIAGNOSTICS](get-diagnostics.md), [CREATE TRIGGER](create-trigger.md)
