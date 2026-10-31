# PREPARE

[Statements](README.md)

## NAME

PREPARE: prepare a statement, by name *(in a program)*

## SYNOPSIS

```
<prepare statement> ::=
    PREPARE <SQL statement name> FROM <SQL statement variable>

<SQL statement name> ::= <statement name> | <extended statement name>

<statement name> ::= <identifier>

<extended statement name> ::= [ GLOBAL | LOCAL ] <parameter name>

<SQL statement variable> ::= <parameter name>
```

## DESCRIPTION

PREPARE prepares the statement a parameter holds, as text, and keeps it
under a name until [DEALLOCATE PREPARE](deallocate-prepare.md) or the
end of the session; [EXECUTE](execute.md) runs it, as often as asked,
[DESCRIBE](describe.md) describes it, and a cursor may read its rows
([ALLOCATE CURSOR](allocate-cursor.md),
[DECLARE CURSOR](declare-cursor.md)). A statement of the name prepared
before is deallocated first.

The statement prepared is one of those a program may prepare: a query,
INSERT, a searched or positioned UPDATE or DELETE, a statement of
schemas, of transactions or of the session. Each `?` in it is a dynamic
parameter, whose type is that of where it stands, as the
[client library](../../client/02-library.md) prepares a statement; it
takes its value as the statement is executed. The statement is checked
against the definitions as it is prepared, and again as it runs: they
may have changed meanwhile. It is kept across transactions. A positioned
UPDATE or DELETE finds its cursor as it is prepared: the cursor of the
name declared, or allocated; one that names no table, `DELETE WHERE
CURRENT OF C`, changes the table the cursor reads.

A name is the session's. A statement name is an identifier; an extended
statement name is the value of a parameter, a character string, its
leading and trailing spaces dropped, which is itself an identifier,
LOCAL if neither GLOBAL nor LOCAL is written. The names of each kind,
and LOCAL and GLOBAL ones, are apart. An identifier that is a
parameter's name, where a statement's name stands, is that parameter, so
the name is an extended one; statement names are best named apart from
parameters.

PREPARE runs only in a program that embeds turborel, through
`Session::execute`, with the program's parameters: its statement is a
parameter's value, never a literal, and direct SQL, and so the client,
has no parameters. It begins a transaction if none is active.

## RULES

1. The statement variable is a parameter, not a literal, of a character
   string type.
2. An extended name's value, its spaces dropped, is an identifier.
3. The statement is one a program may prepare, of no comment, and
   follows the rules of its kind; each `?` takes a type from where it
   stands.
4. Of a positioned UPDATE or DELETE, one cursor of the name is declared
   or allocated; of one that names no table, the cursor reads an
   updatable table.
5. A statement name for which a cursor is declared is of a query.

## PRIVILEGES

Those the statement prepared needs.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The statement variable is a literal, or not a character string (rule 1) |
| 26000 | -921 | An extended name is not an identifier (rule 2) |
| 37000 | -102, -103, -104 | The statement is not one a program may prepare, or breaks a rule or lacks a privilege (rule 3) |
| 34000 | -921 | No cursor of the name of a positioned UPDATE or DELETE (rule 4) |
| 3C000 | -921 | More than one cursor of that name, one declared and one allocated (rule 4) |
| 07005 | -921 | A cursor is declared for the name, and the statement is not a query (rule 5) |

## EXAMPLES

In a program, of the table STAFF of [DECLARE CURSOR](declare-cursor.md),
of rows 1, Ada, 7200; 2, Grace, 8100; and 3, NULL, 6900:

```rust
let s = db.session("ANN");
let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
let mut p = Params::new()
    .declare("TEXT", DataType::Varying { length: 200 }, Value::Null)
    .declare("STMT", DataType::Varying { length: 128 }, Value::Null)
    .declare("MY_ID", integer, Value::Exact(Decimal::new(1, 0)))
    .declare("RAISE", integer, Value::Exact(Decimal::new(100, 0)))
    .declare("MY_NAME", DataType::Varying { length: 30 }, Value::Null)
    .indicator("NAME_IND", 0);
p.set("TEXT", Value::Char("UPDATE STAFF SET SALARY = SALARY + ? WHERE ID = ?".into()));
s.execute("PREPARE RAISE_ONE FROM TEXT", &mut p)?;
s.execute("EXECUTE RAISE_ONE USING RAISE, MY_ID", &mut p)?;      // Ada's SALARY 7300.00
p.set("STMT", Value::Char("NAME_OF".into()));
p.set("TEXT", Value::Char("SELECT NAME FROM STAFF WHERE ID = ?".into()));
s.execute("PREPARE GLOBAL STMT FROM TEXT", &mut p)?;
s.execute("EXECUTE GLOBAL STMT INTO MY_NAME INDICATOR NAME_IND USING MY_ID", &mut p)?;  // Ada
```

Refused, TEXT as shown:

```
PREPARE GONE FROM 'DELETE FROM STAFF'    -103 (2A000): an <SQL statement variable> is not a <literal> [ISO/IEC 9075:1992 §17.6 SR1]
PREPARE GONE FROM TEXT                   DELETE FROM STAF: -103 (37000): table ANN.STAF is not defined [X3.135-1986 §5.4 SR5]
PREPARE GONE FROM TEXT                   SELECT ? FROM STAFF: -103 (37000): a <dynamic parameter specification> here takes no type from where it stands [ISO/IEC 9075:1992 §17.6 GR4]
PREPARE STMT FROM TEXT                   STMT TWO WORDS: -921 (26000): 'TWO WORDS' is not an <identifier> [ISO/IEC 9075:1992 §17.6 GR8]
```

## STANDARD

ISO/IEC 9075:1992 §17.6, of the names of §5.4. SQL-92 makes it no
`<directly executable statement>` [§20.1]. SQL-86 has no dynamic SQL.

## SEE ALSO

[EXECUTE](execute.md), [EXECUTE IMMEDIATE](execute-immediate.md),
[DEALLOCATE PREPARE](deallocate-prepare.md), [DESCRIBE](describe.md),
[ALLOCATE CURSOR](allocate-cursor.md),
[The Client Library](../../client/02-library.md)
