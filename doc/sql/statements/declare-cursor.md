# DECLARE CURSOR

[Statements](README.md)

## NAME

DECLARE CURSOR: declare a cursor *(in a program)*

## SYNOPSIS

```
<declare cursor> ::=
    DECLARE <cursor name> [ INSENSITIVE ] [ SCROLL ] CURSOR
    FOR <cursor specification>

<cursor specification> ::=
    <query expression> [ <order by clause> ] [ <updatability clause> ]

<updatability clause> ::= FOR { READ ONLY | UPDATE [ OF <column name list> ] }
```

The `<query expression>` and `<order by clause>` are those of
[SELECT](select.md).

## DESCRIPTION

A cursor reads a query's rows one at a time, into a program's
parameters: [OPEN](open.md) runs its query, [FETCH](fetch.md) moves it
to a row and reads the row, [CLOSE](close.md) ends it. Through a cursor
on a row, an UPDATE or DELETE ... WHERE CURRENT OF changes that row.

Cursors run only in a program that embeds turborel. A cursor belongs to
the program's module, not to a statement it runs, so it is declared
through the Rust API, `Session::declare_cursor`, not by
`Session::execute`; the statements that use it run by
`Session::execute`, with the program's parameters. Direct SQL, and so
the client, has no cursors: a query's rows are sent whole.

The cursor specification is kept as written, and bound when the cursor
is opened: a name in it that is a parameter's is the parameter, as of
any statement the program runs, so parameters are best named apart from
columns.

SCROLL lets FETCH move the cursor any way; without it, FETCH moves to
the next row alone. INSENSITIVE is taken: a cursor sees the rows as its
transaction's snapshot has them, whatever the cursor is.

**Updatability.** FOR UPDATE makes the cursor updatable, on the columns
named or every column; FOR READ ONLY, read-only. With neither, a cursor
is read-only if it is INSENSITIVE or SCROLL, has ORDER BY, or reads a
read-only table; else updatable on every column.

**Positioned UPDATE and DELETE.**

```
<update statement: positioned> ::=
    UPDATE <table name> SET <set clause> [ { , <set clause> }... ]
    WHERE CURRENT OF <cursor name>

<delete statement: positioned> ::=
    DELETE FROM <table name> WHERE CURRENT OF <cursor name>
```

They change the row the cursor is on, of the table it reads. After a
DELETE, the cursor is on no row until the next FETCH.

## RULES

1. No cursor of the module has the name.
2. An INSENSITIVE cursor is not FOR UPDATE.
3. As it is opened, the specification follows the rules of a query;
   FOR UPDATE is of an updatable query; the columns of FOR UPDATE OF are
   the table's.
4. A positioned UPDATE or DELETE names the cursor's table, and the
   cursor is updatable, open, and on a row; an UPDATE sets only columns
   of its FOR UPDATE.

## PRIVILEGES

As it is opened, SELECT on the tables the query reads. A positioned
UPDATE or DELETE needs UPDATE on the columns set, or DELETE.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | Declared by `Session::execute`, as a statement |
| 2A000 | -103 | A rule above is broken |
| 24000 | -501 | A positioned UPDATE or DELETE of a cursor not open, or on no row |

## EXAMPLES

In a program, of a table STAFF (ID INTEGER PRIMARY KEY, NAME
VARCHAR(30), SALARY DECIMAL(9,2)) of rows 1, Ada, 7200; 2, Grace, 8100;
and 3, NULL, 6900:

```rust
let s = db.session("ANN");
let mut p = Params::new()
    .declare("MY_ID", DataType::Exact { precision: 9, scale: 0 }, Value::Null)
    .declare("MY_NAME", DataType::Varying { length: 30 }, Value::Null)
    .indicator("NAME_IND", 0)
    .declare("FLOOR", DataType::Exact { precision: 9, scale: 0 }, Value::Exact(Decimal::new(7000, 0)));
s.declare_cursor("DECLARE U CURSOR FOR SELECT ID, NAME FROM STAFF WHERE SALARY > FLOOR FOR UPDATE OF SALARY")?;
s.execute("OPEN U", &mut p)?;
s.execute("FETCH U INTO MY_ID, MY_NAME INDICATOR NAME_IND", &mut p)?;  // 1, Ada
s.execute("UPDATE STAFF SET SALARY = SALARY + 100 WHERE CURRENT OF U", &mut p)?;
s.execute("UPDATE STAFF SET NAME = 'x' WHERE CURRENT OF U", &mut p);    // refused
s.execute("FETCH U INTO MY_ID, MY_NAME INDICATOR NAME_IND", &mut p)?;  // 2, Grace
s.execute("DELETE FROM STAFF WHERE CURRENT OF U", &mut p)?;
s.execute("DELETE FROM STAFF WHERE CURRENT OF U", &mut p);              // refused
s.execute("COMMIT", &mut p)?;
```

The refusals, and declaring a name twice:

```
UPDATE STAFF SET NAME = 'x' WHERE CURRENT OF U   -103 (2A000): cursor U is not FOR UPDATE of NAME [ISO/IEC 9075:1992 §13.9 SR8]
DELETE FROM STAFF WHERE CURRENT OF U             -501 (24000): cursor U is not positioned on a row [X3.135-1986 §8.4 GR1]
DECLARE U CURSOR FOR SELECT ID FROM STAFF        -103 (2A000): cursor U is already declared [X3.135-1986 §8.3 SR1]
```

STAFF is left of 1, 7300.00 and 3, 6900.00.

## STANDARD

ISO/IEC 9075:1992 §13.1, §13.6 and §13.9; X3.135-1986 §8.3, §8.4 and
§8.11, whose cursors are neither SCROLL nor INSENSITIVE. Neither
standard makes a cursor's statements `<directly executable statement>`s
[ISO/IEC 9075:1992 §20.1].

## SEE ALSO

[OPEN](open.md), [FETCH](fetch.md), [CLOSE](close.md),
[SELECT](select.md), [UPDATE](update.md), [DELETE](delete.md)
