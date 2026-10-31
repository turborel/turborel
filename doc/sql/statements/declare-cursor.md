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

<dynamic declare cursor> ::=
    DECLARE <cursor name> [ INSENSITIVE ] [ SCROLL ] CURSOR
    FOR <statement name>
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
the client, declares no cursor: DECLARE CURSOR given there is refused
(SQLCODE -102), and OPEN, FETCH, CLOSE and a positioned UPDATE or
DELETE find no cursor of the name (SQLCODE -103). A query given there
is read by a cursor of its own all the same, unnamed: its rows are sent
a batch at a time as they are fetched, never held whole first
([The Embedded Library](../../client/03-embedding.md),
[The Client Library](../../client/02-library.md)).

The cursor specification is kept as written, and bound when the cursor
is opened: a name in it that is a parameter's is the parameter, as of
any statement the program runs, so parameters are best named apart from
columns.

**A dynamic cursor.** Declared FOR a statement name, the cursor's
specification is the query [PREPARE](prepare.md) prepares by that name,
read as the cursor is opened: [OPEN](open.md) gives its dynamic
parameters their values, USING parameters or an SQL descriptor area,
and [FETCH](fetch.md) puts its rows into targets or into a descriptor
area. The name may be prepared before the cursor is opened, and again,
of a query, once it is closed. [ALLOCATE CURSOR](allocate-cursor.md)
allocates a cursor of a query prepared by an extended name, as a
statement.

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
DELETE, the cursor is on no row until the next FETCH. Prepared, either
may name no table, `DELETE WHERE CURRENT OF U`: it is of the table the
cursor reads, a cursor declared or allocated, which is found as the
statement is prepared.

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

A dynamic cursor, of the parameters of [PREPARE](prepare.md), FLOOR
holding 7000, and of a descriptor area ROW and N, exact:

```rust
s.declare_cursor("DECLARE D SCROLL CURSOR FOR NAME_LIST")?;
s.execute("OPEN D USING FLOOR", &mut p);                              // refused: not prepared
p.set("TEXT", Value::Char("SELECT ID, NAME FROM STAFF WHERE SALARY < ? ORDER BY ID".into()));
s.execute("PREPARE NAME_LIST FROM TEXT", &mut p)?;
s.execute("OPEN D USING FLOOR", &mut p)?;
s.execute("FETCH LAST FROM D INTO MY_ID, MY_NAME INDICATOR NAME_IND", &mut p)?;   // 3, NAME_IND = -1
s.execute("ALLOCATE DESCRIPTOR 'ROW'", &mut p)?;
s.execute("DESCRIBE NAME_LIST USING SQL DESCRIPTOR 'ROW'", &mut p)?;
s.execute("SET DESCRIPTOR 'ROW' VALUE 1 TYPE = 2, PRECISION = 9", &mut p)?;
s.execute("FETCH FIRST FROM D INTO SQL DESCRIPTOR 'ROW'", &mut p)?;
s.execute("GET DESCRIPTOR 'ROW' VALUE 1 N = DATA", &mut p)?;          // 3
s.execute("CLOSE D", &mut p)?;
```

A positioned UPDATE prepared, of no table, RAISE holding 100:

```rust
s.declare_cursor("DECLARE U CURSOR FOR SELECT ID, SALARY FROM STAFF FOR UPDATE OF SALARY")?;
p.set("TEXT", Value::Char("UPDATE SET SALARY = SALARY + ? WHERE CURRENT OF U".into()));
s.execute("PREPARE PAY_RISE FROM TEXT", &mut p)?;
s.execute("OPEN U", &mut p)?;
s.execute("FETCH U INTO MY_ID, N", &mut p)?;
s.execute("EXECUTE PAY_RISE USING RAISE", &mut p)?;                   // Ada's SALARY 7300.00
```

## STANDARD

ISO/IEC 9075:1992 §13.1, §13.6 and §13.9; of a dynamic cursor, §17.12,
§17.19 and §17.20; X3.135-1986 §8.3, §8.4 and
§8.11, whose cursors are neither SCROLL nor INSENSITIVE. Neither
standard makes a cursor's statements `<directly executable statement>`s
[ISO/IEC 9075:1992 §20.1].

## SEE ALSO

[OPEN](open.md), [FETCH](fetch.md), [CLOSE](close.md),
[SELECT](select.md), [UPDATE](update.md), [DELETE](delete.md),
[PREPARE](prepare.md), [ALLOCATE CURSOR](allocate-cursor.md)
