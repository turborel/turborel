# The Embedded Library

[Contents](README.md)

The crate `turborel` is the database itself, as a library: a Rust
program that embeds it holds a database in its own process, and runs SQL
through it with no server between. What a server's client does over a
connection, the program does by calls: a `Database`, and a `Session` of
it for each authorization identifier that runs statements. Each example
below is a whole program, and runs as printed.

## The database

`Database::new` makes a database held in memory, which ends with the
program. `Database::open` opens the one kept in a directory, making it
if there is none, and recovering it if it was not closed
([Databases](../operator/03-databases.md)); `open_files` does the same
through a `FileSystem` and `OpenOptions` of the program's own. A
database kept in a directory is closed by `close`, with no
transaction open: it writes a checkpoint and gives the directory up.
`export_manifest` lists the durable files of a database kept in a
directory as its last checkpoint left them, each with its length and
owner, but its log's and its catalog's: what a copy of the database
needs besides its log and its catalog. A file in its directory of no
kind the database knows is refused (SQLCODE -910), as a copy would
leave it behind.

A `Database` is a handle: cloned, it is the same database, and it may
be shared by any number of threads. Its settings are set by
`set_setting`, as SET DATABASE sets them, a setting read as the
database opens set before `open_files`
([Settings](../appendix/c-settings.md)).

```rust
use std::path::PathBuf;

use turborel::db::Database;
use turborel::value::Value;

fn main() -> Result<(), turborel::error::SqlError> {
    let dir: PathBuf = std::env::temp_dir().join(format!("turborel-manual-{}", std::process::id()));
    let db = Database::open(&dir)?;
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)")?;
    db.session("ANN").run("INSERT INTO T VALUES (1)")?;
    db.close()?;
    drop(db);
    let again = Database::open(&dir)?;
    let page = again.setting("STORAGE.PAGE_BYTES")?;
    assert_eq!(page.to_string(), "8192");
    assert!(matches!(page, Value::Exact(_)));
    again.close()?;
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
```

The session that inserted the row was dropped at the end of its
statement's line, its transaction committed as SESSION.COMMIT_AT_END says, so the
row was kept.

## Sessions

`Database::schema` runs a `<schema definition>` as the database's
operator, for any owner. `Database::session` gives a session acting as
an authorization identifier: what it may do is what that identifier is
granted, as for a server's client connected as that user. `run` runs a
statement, any but a query, which is read by a cursor, and gives its
`Completion`: `Success`, `NoData`, or a
`Warning` of its SQLSTATE. The first statement that reads or changes
data begins a transaction; `commit` and `rollback` end it, as COMMIT
and ROLLBACK do. A `commit` that fails with SQLCODE -922 is in doubt:
its commit record could not be synced, so whether it committed is known
only once the database is opened again; it is not to be run again
before then, as it may have committed. A session dropped ends its transaction as the setting
SESSION.COMMIT_AT_END says: committed, by default.

A session is used by one thread at a time; it may move to another
between calls. Each thread of a program has its own sessions.

`Database::set_owner` makes a user the database's owner, unchecked, as
the program holding the database may. `Session::set_owner` does it, as
[ALTER DATABASE](../sql/statements/alter-database.md) does, for a
session of the owner alone, outside a transaction (SQLCODE -104 for
another's, -917 in a transaction, -103 for a name not a user's): the
owner hands the database on, and its sessions go on as the owner no
more ([Users](../operator/05-users.md)).

`reseal_table("AUTH.TABLE")`, as `ALTER TABLE ... RESEAL` does, seals
a table's rows again through the
encodings its option ENCODINGS names now, in the session's transaction,
as its schema's owner alone may: committed and checkpointed, nothing of
the table is read through a key it named before
([Encryption](../operator/07-encryption.md#retiring-a-key)).

A statement that fails gives an `SqlError`: its `code`, `sqlcode()`
and `sqlstate()`, its message, the rule of the standard it cites, and
where in the statement the fault is. Shown, it is as the client shows
it ([Appendix A](../appendix/a-sqlstate.md)).

```rust
use turborel::db::{Completion, Database};

fn main() -> Result<(), turborel::error::SqlError> {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30))")?;
    let ann = db.session("ANN");
    assert_eq!(ann.run("INSERT INTO STAFF VALUES (1, 'Ada'), (2, 'Grace')")?, Completion::Success);
    assert_eq!(ann.run("DELETE FROM STAFF WHERE ID = 9")?, Completion::NoData);
    ann.commit()?;
    let refused = ann.run("DELETE FROM NOPE").unwrap_err();
    assert_eq!((refused.sqlcode(), refused.sqlstate().to_string()), (-103, "2A000".to_owned()));
    assert_eq!(
        refused.to_string(),
        "SQLCODE -103 (SQLSTATE 2A000): table ANN.NOPE is not defined [X3.135-1986 §5.4 SR5] (at character 12)"
    );
    Ok(())
}
```

## Reading rows

`open_cursor` runs a query and gives a `Cursor` of its rows: its
`columns`, each of a name, if it has one, and a type; and `fetch`, the
next row, a `Vec` of `Value`s, or none after the last. Rows are read as
they are fetched, in the session's transaction, not gathered first.
`direct` runs any statement as a server runs its client's: a query
gives a cursor, `Direct::Rows`; any other statement its completion,
`Direct::Done`.

```rust
use turborel::db::{Database, Direct};
use turborel::value::Value;

fn main() -> Result<(), turborel::error::SqlError> {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(30))")?;
    let ann = db.session("ANN");
    ann.run("INSERT INTO STAFF VALUES (1, 'Ada'), (2, 'Grace'), (3, NULL)")?;
    let mut rows = ann.open_cursor("SELECT ID, NAME FROM STAFF ORDER BY ID")?;
    assert_eq!(rows.columns[1].name.as_deref(), Some("NAME"));
    let mut names = Vec::new();
    while let Some(row) = rows.fetch()? {
        names.push(if matches!(row[1], Value::Null) { "null".to_owned() } else { row[1].to_string() });
    }
    assert_eq!(names, ["Ada", "Grace", "null"]);
    assert!(matches!(ann.direct("UPDATE STAFF SET NAME = 'Edsger' WHERE ID = 3")?, Direct::Done(_)));
    assert!(matches!(ann.direct("VALUES 1")?, Direct::Rows(_)));
    Ok(())
}
```

## Parameters

A program's own values reach a statement as parameters, as SQL-86's
module language passes them: `Params` declares each, by name, of a type
and a value, and may give it an indicator, which a null value read into
it sets to -1. `execute` runs a statement with them: a name in it that
is a parameter's is the parameter, so parameters are best named apart
from columns. SELECT ... INTO and FETCH ... INTO set them; `value`
reads one back.

```rust
use turborel::db::Database;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

fn main() -> Result<(), turborel::error::SqlError> {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(30))")?;
    let ann = db.session("ANN");
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let mut p = Params::new()
        .declare("NEW_ID", integer, Value::Exact(Decimal::new(4, 0)))
        .declare("NEW_NAME", DataType::Varying { length: 30 }, Value::Char("Barbara".into()))
        .declare("OUT_NAME", DataType::Varying { length: 30 }, Value::Null)
        .indicator("OUT_IND", 0);
    ann.execute("INSERT INTO STAFF VALUES (NEW_ID, NEW_NAME)", &mut p)?;
    ann.execute("SELECT NAME INTO OUT_NAME INDICATOR OUT_IND FROM STAFF WHERE ID = NEW_ID", &mut p)?;
    assert_eq!(p.value("OUT_NAME").to_string(), "Barbara");
    assert_eq!(p.indicator_value("OUT_IND"), Some(0));
    Ok(())
}
```

## Cursors

A cursor of the program's is declared by `declare_cursor`, not run as a
statement; OPEN, FETCH, CLOSE and UPDATE or DELETE ... WHERE CURRENT OF
then run by `execute`, with the program's parameters
([DECLARE CURSOR](../sql/statements/declare-cursor.md)). A FETCH past
the last row completes with no data.

```rust
use turborel::db::{Completion, Database};
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

fn main() -> Result<(), turborel::error::SqlError> {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER, SALARY DECIMAL(9,2))")?;
    let ann = db.session("ANN");
    ann.run("INSERT INTO STAFF VALUES (1, 7200), (2, 8100)")?;
    let mut p = Params::new().declare("MY_ID", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, Value::Null);
    ann.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM STAFF WHERE ID > 1 FOR UPDATE OF SALARY")?;
    ann.execute("OPEN C", &mut p)?;
    assert_eq!(ann.execute("FETCH C INTO MY_ID", &mut p)?, Completion::Success);
    assert_eq!(p.value("MY_ID").to_string(), "2");
    ann.execute("UPDATE STAFF SET SALARY = SALARY + 100 WHERE CURRENT OF C", &mut p)?;
    assert_eq!(ann.execute("FETCH C INTO MY_ID", &mut p)?, Completion::NoData);
    ann.execute("CLOSE C", &mut p)?;
    let mut rows = ann.open_cursor("SELECT SALARY FROM STAFF WHERE ID = 2")?;
    assert_eq!(rows.fetch()?.map(|r| r[0].to_string()), Some("8200.00".to_owned()));
    Ok(())
}
```

## Prepared statements

`prepare` prepares a statement once, its dynamic parameters written `?`:
`parameters` gives the type each takes, `columns` a query's columns.
`execute_prepared` runs it with values for them, in order;
`open_prepared` runs a prepared query, and gives its cursor. A prepared
statement stays valid across transactions. A query, an INSERT, and a
searched or positioned UPDATE or DELETE keep their plan between
executions, a query also when SQL text opens it as a dynamic cursor:
each execution puts its values in the kept plan and chooses indexes by
them. The plan is made again when the definitions change (a table
altered, an index made or dropped, a local temporary table declared, a
privilege granted or revoked, the transaction's own uncommitted changes
among them), when a setting of the database or
of the session is set, or when the session's schema, authorization
identifier or time zone changes; a plan that holds the statement's
instant, of CURRENT_TIMESTAMP in the statement or a view it reads, or of
a DEFAULT it writes, is made at each execution. `plans_made` counts the plans a statement's
executions made.

```rust
use turborel::db::Database;
use turborel::value::{Decimal, Value};

fn main() -> Result<(), turborel::error::SqlError> {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(30))")?;
    let ann = db.session("ANN");
    let insert = ann.prepare("INSERT INTO STAFF VALUES (?, ?)")?;
    assert_eq!(insert.parameters().len(), 2);
    for (id, name) in [(1, "Ada"), (2, "Grace")] {
        ann.execute_prepared(&insert, &[Value::Exact(Decimal::new(id, 0)), Value::Char(name.into())])?;
    }
    let by_id = ann.prepare("SELECT NAME FROM STAFF WHERE ID = ?")?;
    let mut rows = ann.open_prepared(&by_id, &[Value::Exact(Decimal::new(2, 0))])?;
    assert_eq!(rows.fetch()?.map(|r| r[0].to_string()), Some("Grace".to_owned()));
    assert_eq!(insert.plans_made(), 1);
    Ok(())
}
```

A statement may be prepared in SQL text too, by name, its text and its
values the program's parameters, as `execute` runs any statement:
[PREPARE](../sql/statements/prepare.md), [EXECUTE](../sql/statements/execute.md)
and the other statements of dynamic SQL, SQL descriptor areas among
them ([DESCRIBE](../sql/statements/describe.md)).

```rust
use turborel::db::Database;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

fn main() -> Result<(), turborel::error::SqlError> {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(30))")?;
    let ann = db.session("ANN");
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let mut p = Params::new()
        .declare("TEXT", DataType::Varying { length: 200 }, Value::Char("INSERT INTO STAFF VALUES (?, ?)".into()))
        .declare("MY_ID", integer, Value::Exact(Decimal::new(1, 0)))
        .declare("MY_NAME", DataType::Varying { length: 30 }, Value::Char("Ada".into()));
    ann.execute("PREPARE ADD_ONE FROM TEXT", &mut p)?;
    ann.execute("EXECUTE ADD_ONE USING MY_ID, MY_NAME", &mut p)?;
    p.set("TEXT", Value::Char("SELECT NAME FROM STAFF WHERE ID = ?".into()));
    ann.execute("PREPARE NAME_OF FROM TEXT", &mut p)?;
    p.set("MY_NAME", Value::Null);
    ann.execute("EXECUTE NAME_OF INTO MY_NAME USING MY_ID", &mut p)?;
    assert_eq!(p.value("MY_NAME").to_string(), "Ada");
    Ok(())
}
```

## Functions

A program may add scalar functions, and set functions, of its own:
`register_scalar_function` takes an implementation of the trait
`ScalarFunction`, of a name, the type of its result for the types of
its arguments, and the value of a call; `register_aggregate_function` a
set function's, and replaces a standard one of its name. A scalar
function is one for each name, whatever its arguments' types: a second
of a name already registered, a standard one's such as UPPER too, is
refused (SQLCODE -910), and the first stays. A call,
`NAME(arguments)`, is syntax the standards do not have *(extension)*:
a session takes it only once its setting SQL.FUNCTION_CALLS is ON, and
refuses it until then, as SQL does (SQLCODE -102).

```rust
use turborel::db::Database;
use turborel::error::{Code, Result, fail};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Value};

/// TWICE(n): n added to itself.
struct Twice;

impl ScalarFunction for Twice {
    fn name(&self) -> &str {
        "TWICE"
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        match args {
            [exact @ DataType::Exact { .. }] => Ok(*exact),
            _ => fail(Code::SyntaxRule, "TWICE takes one exact number"),
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args {
            [Value::Exact(n)] => Ok(Value::Exact(n.checked_add(*n)?)),
            _ => Ok(Value::Null),
        }
    }
}

fn main() -> Result<()> {
    let db = Database::new();
    db.register_scalar_function(Box::new(Twice))?;
    let ann = db.session("ANN");
    assert_eq!(ann.run("VALUES TWICE(21)").unwrap_err().sqlcode(), -102);
    ann.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".to_owned()))?;
    let mut rows = ann.open_cursor("VALUES TWICE(21)")?;
    assert_eq!(rows.fetch()?.map(|r| r[0].to_string()), Some("42".to_owned()));
    Ok(())
}
```

A function may also declare the type of each argument, by
`argument_type`: an argument of another type is then stored in it as a
value is in a column of that type, a number given the declared scale, a
character string made a value of a type beyond the standard that takes
one; one that cannot be is refused before the statement runs (SQLCODE
-103); `return_type` is told the declared types,
and a `?` there takes the declared type. A function declaring none is
given its arguments as they are. One function is registered under a
name, whatever its arguments.

```rust
use turborel::db::Database;
use turborel::error::{Code, Result, fail};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Decimal, Spelled, Value};

/// WIDTH(s): the characters of s, stored in a CHARACTER(5).
struct Width;

const FIVE: DataType = DataType::Character { length: 5 };

impl ScalarFunction for Width {
    fn name(&self) -> &str {
        "WIDTH"
    }
    fn argument_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<DataType> {
        Some(FIVE)
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        match args {
            [FIVE] => Ok(DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }),
            _ => fail(Code::SyntaxRule, "WIDTH takes one string"),
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args {
            [Value::Char(s)] => Ok(Value::Exact(Decimal::new(s.chars().count().try_into().unwrap_or(0), 0))),
            _ => Ok(Value::Null),
        }
    }
}

fn main() -> Result<()> {
    let db = Database::new();
    db.register_scalar_function(Box::new(Width))?;
    let ann = db.session("ANN");
    ann.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".to_owned()))?;
    let mut rows = ann.open_cursor("VALUES WIDTH('abc')")?;
    assert_eq!(rows.fetch()?.map(|r| r[0].to_string()), Some("5".to_owned()));
    assert_eq!(ann.open_cursor("VALUES WIDTH(1)").err().map(|e| e.sqlcode()), Some(-103));
    Ok(())
}
```

A function is called from any thread a statement runs on, so it is
`Send` and `Sync`, and guards any state of its own.

## Engines and other plug-ins

Each extension point is a trait, whose implementation the program
registers on the database, by name, before the database opens its
files, so the definitions that name one find it:

| Registered by | What |
|---|---|
| `register_engine` | a storage engine, `StorageEngine`: what keeps a table's rows, named by CREATE TABLE ... USING, and by the setting TABLES.ENGINE |
| `register_index_method` | an index method, `IndexMethod`, named by CREATE INDEX ... USING |
| `register_type` | a data type beyond the standard's, `SqlType`, named in a column definition and CAST: a value of it is CAST from a character string, and a character string is stored in it as in a character column only if the type says it takes one (`assigns_text`); no privilege is needed to use it |
| `register_scalar_function`, `register_aggregate_function` | functions, as above |
| `register_encoding` | an encoding of files or tables' rows, `Encoding`, named by STORAGE.ENCODINGS and a table's ENCODINGS |
| `register_hash_function` | a hash function for checksums, `HashFunction`, named by STORAGE.CHECKSUM |
| `register_system_table` | a table of the Information Schema's kind, `SystemTable` |
| `set_diagnostics` | where the database's events go, `Diagnostics` ([Diagnostics](../operator/08-diagnostics.md)); `tell` gives it one of the program's own, as a server's, those below DIAGNOSTICS.LEVEL dropped |

A plug-in of any kind above, and a catalog's or a log's storage, the
credentials, a server's connections, a spill, a log (`set_log`) and a
clock (`set_clock`), may declare settings of the database's, under its
own name (a log, a sink or a clock giving none is named `log_plugin`,
`diagnostics_plugin` or `clock_plugin`), as the column store declares
COLUMNSTORE.HEADER_CACHE_BYTES: it gives itself as its `Configurable`
(`configurable`), whose `settings` it declares. It is told their values
as it is registered, as each changes and as the database opens
(`Configurable::configure`), and never calls back into the database to
read them; a setting outside its name, or a built-in one's, is refused,
and the plug-in not registered. `with_optimizer` adds rules to the optimizer (`Rule`),
each with its setting, OPTIMIZER and its name, which turns it on or
off, as the built-in rules' do ([Settings](../appendix/c-settings.md)).

The built-in engines are `rowstore`, the default, and `columnstore`
([Table Options](../appendix/g-table-options.md)); the built-in index
methods `btree` and `hash`. Each trait's contract, what the database
calls and what an implementation must keep, is in the crate's own
documentation, of the module the trait is in.

```rust
use turborel::db::Database;

fn main() -> Result<(), turborel::error::SqlError> {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE EVENTS (ID INTEGER, KIND CHAR(8)) USING columnstore")?;
    let ann = db.session("ANN");
    ann.run("INSERT INTO EVENTS VALUES (1, 'start')")?;
    let engine = "SELECT DISTINCT ENGINE_NAME FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = 'EVENTS'";
    let mut rows = ann.open_cursor(engine)?;
    assert_eq!(rows.fetch()?.map(|r| r[0].to_string().trim_end().to_owned()), Some("columnstore".to_owned()));
    Ok(())
}
```

## See also

[The Client Library](02-library.md), to connect to a server instead;
[Statements](../sql/statements/README.md); the crate's documentation,
`cargo doc -p turborel`.
