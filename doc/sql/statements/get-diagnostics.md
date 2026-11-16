# GET DIAGNOSTICS

[Statements](README.md)

## NAME

GET DIAGNOSTICS: read the diagnostics area *(in a program)*

## SYNOPSIS

```
<get diagnostics statement> ::=
    GET DIAGNOSTICS { <statement information> | <condition information> | <all information> }

<statement information> ::=
    <parameter name> = <statement information item name>
    [ { , <parameter name> = <statement information item name> }... ]

<condition information> ::=
    { CONDITION | EXCEPTION } <condition number>
    <parameter name> = <condition information item name>
    [ { , <parameter name> = <condition information item name> }... ]

<all information> ::=
    <parameter name> = ALL [ STATEMENT | CONDITION [ <condition number> ] ]

<condition number> ::= <parameter name> | <literal>
```

## DESCRIPTION

GET DIAGNOSTICS copies what the session's last statement left in its
diagnostics area into parameters. It runs only in a program that embeds
turborel, through the Rust API: its targets are parameters, which direct
SQL, and so the client, has none of. The client shows each statement's
outcome itself.

The area holds one condition after each statement: its completion,
SQLSTATE 00000, or the warning or exception it raised. GET DIAGNOSTICS
changes nothing of it, so it may be read again.

The statement information items:

| Item | Type | Value |
|---|---|---|
| NUMBER | exact | The number of conditions: 0 before any statement, else 1 |
| MORE | character | N, or Y if the statement raised a condition the area does not hold |
| COMMAND_FUNCTION | character | The statement, as `INSERT`, `UPDATE WHERE`, `DELETE WHERE`, `COMMIT WORK`, `DROP VIEW`; empty for a statement that did not parse |
| COMMAND_FUNCTION_CODE | exact | Its code, as 50 for INSERT; 0 for a statement that did not parse |
| DYNAMIC_FUNCTION | character | Of EXECUTE, the statement prepared; else empty |
| DYNAMIC_FUNCTION_CODE | exact | Its code; else 0 |
| ROW_COUNT | exact | The rows an INSERT, a searched UPDATE or DELETE changed, and those a MERGE read (see [MERGE](merge.md)) |
| TRANSACTIONS_COMMITTED | exact | The transactions the statement committed, 0 or 1 |
| TRANSACTIONS_ROLLED_BACK | exact | The transactions the statement rolled back, a COMMIT that failed among them, 0 or 1 |
| TRANSACTION_ACTIVE | exact | 1 if a transaction is active, else 0 |

Each statement's code is the standard's. SELECT has three, 65 for SELECT
... INTO, 21 for a cursor the Rust API reads whole and 41 for a statement
prepared and executed INTO targets. A cursor specification prepared and
opened is `SELECT CURSOR`, code 85. A positioned UPDATE or DELETE of a
dynamic cursor is coded 42 or 38, and one prepared 55 or 54, though both
are named `DYNAMIC UPDATE CURSOR` or `DYNAMIC DELETE CURSOR`. turborel's
own statements have negative codes.

| Statement | Code |
|---|---|
| DECLARE LOCAL TEMPORARY TABLE | -1 |
| SET of a setting | -3 |
| SET DATABASE | -4 |
| CREATE INDEX, ALTER INDEX, DROP INDEX | -5, -6, -7 |
| CREATE USER, ALTER USER, DROP USER | -8, -9, -10 |
| ALTER DATABASE | -11 |
| END CONNECTION, CANCEL CONNECTION | -12, -13 |

The condition information items, of condition 1:

| Item | Type | Value |
|---|---|---|
| CONDITION_NUMBER | exact | 1 |
| RETURNED_SQLSTATE | character | The SQLSTATE |
| CLASS_ORIGIN, SUBCLASS_ORIGIN | character | `ISO 9075` for the standard's classes; `TURBOREL` for turborel's own, as TB911 |
| MESSAGE_TEXT | character | The message, as the client shows it, without its position |
| MESSAGE_LENGTH, MESSAGE_OCTET_LENGTH | exact | Its length in characters, and in octets |
| CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME | character | The constraint or assertion broken |
| CATALOG_NAME, SCHEMA_NAME, TABLE_NAME, COLUMN_NAME | character | The table the condition is about, and the column, where it is about one |
| CURSOR_NAME | character | The cursor the condition is about |
| SERVER_NAME, CONNECTION_NAME | character | `DEFAULT`: a session embedded in a program is on the default connection |
| ROUTINE_CATALOG, ROUTINE_SCHEMA, ROUTINE_NAME, SPECIFIC_NAME | character | Of SQLSTATE 2F005, or of an argument stored in its parameter, the function or procedure; else empty. A condition of a distinct type's function is one of the CAST it stands for |
| PARAMETER_MODE, PARAMETER_NAME | character | Of an argument stored in its parameter, the parameter's mode and name; else empty |
| PARAMETER_ORDINAL_POSITION | exact | Of an argument stored in its parameter, the parameter's position, from 1; else 0 |
| TRIGGER_CATALOG, TRIGGER_SCHEMA, TRIGGER_NAME | character | Of SQLSTATE 09000, the trigger whose action failed; else empty |

An item that does not apply is empty, or 0. Of SQLSTATE 09000, CATALOG_NAME,
SCHEMA_NAME and TABLE_NAME are the trigger's table, and MORE is Y: the
condition the action failed with is left out, its SQLSTATE and message
within MESSAGE_TEXT. A name of a table the
session holds no privilege on is withheld, but for a syntax error or
access rule violation (SQLSTATE class 42 or 2A). An assertion's name is
withheld from a session that does not own its schema. An assertion broken
also names the one table the statement modified of those it reads.

A syntax error or access rule violation names the table that caused
it: a table the statement names that is not defined, or has no column
it names, or that a privilege is refused on, or that a foreign key
references; else the table or view the statement inserts into, updates,
deletes from, creates, alters, drops, indexes or grants or revokes on.
It names the column where a privilege on it is refused, or the table
has no column of the name. A declared local temporary table's
CATALOG_NAME is one space, its SCHEMA_NAME `MODULE`.

ALL puts every item into one character string, each as `NAME = value`,
joined by semicolons, an item whose value is empty left out. The
statement items come first, in the order of the first table, then of
each condition its CONDITION_NUMBER and items, in the order of the
grammar. ALL STATEMENT gives the statement items alone, ALL CONDITION the
conditions' alone, and ALL CONDITION with a number that condition's
items alone, without its CONDITION_NUMBER.

Each item is assigned to its target as FETCH assigns a column: a number
to a target of any numeric type, a character string to one of any
character string type, cut to fit with a warning.

## RULES

1. Each target is a parameter declared, of a type the item's value may
   be assigned to: numeric for a number, a character string for a
   character string.
2. ALL's target is of a character string type.
3. The condition number is exact with scale 0, and 1 to the number of
   conditions there are.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | An item's name is not one of those above |
| 2A000 | -103 | A target is not a parameter declared, or not of a type its item may be assigned to; ALL's target is not a character string |
| 35000 | -301 | There is no condition of the number |
| 22003 | -301 | A number too large for its target |
| 01004 | 0 | Not an error: a warning, a value cut to fit its target, as [FETCH](fetch.md) cuts it; the area is left as it was |

## EXAMPLES

In a program, of a table STAFF (ID INTEGER PRIMARY KEY, NAME
VARCHAR(30), CONSTRAINT PAID CHECK (ID > 0)):

```rust
let s = db.session("ANN");
let mut p = Params::new()
    .declare("N", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null)
    .declare("CMD", DataType::varying(128), Value::Null)
    .declare("STATE", DataType::character(5), Value::Null)
    .declare("NAME", DataType::varying(128), Value::Null)
    .declare("REPORT", DataType::varying(1000), Value::Null);
s.run("INSERT INTO STAFF VALUES (1, 'Ada'), (2, 'Grace')")?;
s.execute("GET DIAGNOSTICS N = ROW_COUNT, CMD = COMMAND_FUNCTION", &mut p)?;
// N = 2, CMD = INSERT
s.execute("GET DIAGNOSTICS REPORT = ALL STATEMENT", &mut p)?;
// REPORT = NUMBER = 1;MORE = N;COMMAND_FUNCTION = INSERT;COMMAND_FUNCTION_CODE = 50;DYNAMIC_FUNCTION_CODE = 0;
//          ROW_COUNT = 2;TRANSACTIONS_COMMITTED = 0;TRANSACTIONS_ROLLED_BACK = 0;TRANSACTION_ACTIVE = 1
let _refused = s.run("INSERT INTO STAFF VALUES (-1, 'Edsger')");
s.execute("GET DIAGNOSTICS CONDITION 1 STATE = RETURNED_SQLSTATE, NAME = CONSTRAINT_NAME", &mut p)?;
// STATE = 23000, NAME = PAID
s.commit()?;
s.execute("GET DIAGNOSTICS N = TRANSACTIONS_COMMITTED", &mut p)?;
// N = 1
```

Refused:

```
GET DIAGNOSTICS X = NUMBER                              -103 (2A000): parameter X is not declared [X3.135-1986 §7.3 SR3]
GET DIAGNOSTICS N = NOPE                                -102 (2A000): expected a <statement information item name>, found NOPE
GET DIAGNOSTICS N = MORE                                -103 (2A000): a CHARACTER VARYING(32767) item cannot be assigned to N (NUMERIC(9,0)) [ISO/IEC 9075-2:2023 §23.1 SR1, §9.1 SR3]
GET DIAGNOSTICS N = ALL                                 -103 (2A000): target N of ALL is NUMERIC(9,0), not a character string [ISO/IEC 9075-2:2023 §23.1 SR2]
GET DIAGNOSTICS CONDITION 5 STATE = RETURNED_SQLSTATE   -301 (35000): there is no condition 5: the area holds 1 [ISO/IEC 9075:1992 §18.1 GR2]
```

## STANDARD

ISO/IEC 9075:1992 §18.1, of the diagnostics area of §4.19. SQL-92 does
not make it a `<directly executable statement>` [§20.1]. SQL-86 has no
diagnostics area. ISO/IEC 9075-2:2023 §23.1 adds CONDITION in
EXCEPTION's place, the codes and the items of routines and triggers,
targets of another type (Feature F122), ALL (Feature F123) and the
transaction counts (Feature T511).

## SEE ALSO

[Errors](../../user/10-errors.md), [SQLSTATE](../../appendix/a-sqlstate.md),
[The Rust Library](../../client/02-library.md)
