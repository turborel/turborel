# GET DIAGNOSTICS

[Statements](README.md)

## NAME

GET DIAGNOSTICS: read the diagnostics area *(in a program)*

## SYNOPSIS

```
<get diagnostics statement> ::=
    GET DIAGNOSTICS { <statement information> | <condition information> }

<statement information> ::=
    <parameter name> = <statement information item name>
    [ { , <parameter name> = <statement information item name> }... ]

<condition information> ::=
    EXCEPTION <condition number>
    <parameter name> = <condition information item name>
    [ { , <parameter name> = <condition information item name> }... ]

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
| MORE | character | N: no condition was left out |
| COMMAND_FUNCTION | character | The statement, as `INSERT`, `UPDATE WHERE`, `DELETE WHERE`, `COMMIT WORK`, `DROP VIEW`; empty for a statement that did not parse |
| DYNAMIC_FUNCTION | character | Of EXECUTE, the statement prepared; else empty |
| ROW_COUNT | exact | The rows an INSERT, a searched UPDATE or DELETE changed |

The condition information items, of condition 1:

| Item | Type | Value |
|---|---|---|
| CONDITION_NUMBER | exact | 1 |
| RETURNED_SQLSTATE | character | The SQLSTATE |
| CLASS_ORIGIN, SUBCLASS_ORIGIN | character | `ISO 9075` for the standard's classes; `TURBOREL` for turborel's own, as TB911 |
| MESSAGE_TEXT | character | The message, as the client shows it, without its position |
| MESSAGE_LENGTH, MESSAGE_OCTET_LENGTH | exact | Its length in characters, and in octets |
| CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME | character | The constraint broken |
| CATALOG_NAME, SCHEMA_NAME, TABLE_NAME, COLUMN_NAME | character | The table the condition is about, and the column, where it is about one |
| CURSOR_NAME | character | The cursor the condition is about |
| SERVER_NAME, CONNECTION_NAME | character | Empty |

An item that does not apply is empty, or 0. A name of a table the
session holds no privilege on is withheld.

## RULES

1. Each target is a parameter declared, of a type the item's value may
   be assigned to.
2. The condition number is 1 to the number of conditions there are.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | An item's name is not one of those above |
| 2A000 | -103 | A target is not a parameter declared |
| 35000 | -301 | There is no condition of the number |
| 01004 | 0 | Not an error: a warning, a value cut to fit its target, as [FETCH](fetch.md) cuts it; the area is left as it was |

## EXAMPLES

In a program, of a table STAFF (ID INTEGER PRIMARY KEY, NAME
VARCHAR(30), CONSTRAINT PAID CHECK (ID > 0)):

```rust
let s = db.session("ANN");
let mut p = Params::new()
    .declare("N", DataType::Exact { precision: 9, scale: 0 }, Value::Null)
    .declare("CMD", DataType::Varying { length: 128 }, Value::Null)
    .declare("STATE", DataType::Character { length: 5 }, Value::Null)
    .declare("NAME", DataType::Varying { length: 128 }, Value::Null);
s.run("INSERT INTO STAFF VALUES (1, 'Ada'), (2, 'Grace')")?;
s.execute("GET DIAGNOSTICS N = ROW_COUNT, CMD = COMMAND_FUNCTION", &mut p)?;
// N = 2, CMD = INSERT
let _refused = s.run("INSERT INTO STAFF VALUES (-1, 'Edsger')");
s.execute("GET DIAGNOSTICS EXCEPTION 1 STATE = RETURNED_SQLSTATE, NAME = CONSTRAINT_NAME", &mut p)?;
// STATE = 23000, NAME = PAID
```

Refused:

```
GET DIAGNOSTICS X = NUMBER                              -103 (2A000): parameter X is not declared [X3.135-1986 §7.3 SR3]
GET DIAGNOSTICS N = NOPE                                -102 (2A000): expected a <statement information item name>, found NOPE
GET DIAGNOSTICS EXCEPTION 5 STATE = RETURNED_SQLSTATE   -301 (35000): there is no condition 5: the area holds 1 [ISO/IEC 9075:1992 §18.1 GR2]
```

## STANDARD

ISO/IEC 9075:1992 §18.1, of the diagnostics area of §4.19. SQL-92 does
not make it a `<directly executable statement>` [§20.1]. Its names of a
table are only where a privilege is refused, a table is not defined, or
a table named has no column named, and of a column only in the last
case or of a privilege on a column [§18.1 GR3g]. SQL-86 has no
diagnostics area.

## SEE ALSO

[Errors](../../user/10-errors.md), [SQLSTATE](../../appendix/a-sqlstate.md),
[The Rust Library](../../client/02-library.md)
