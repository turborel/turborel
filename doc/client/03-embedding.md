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
        .declare("NEW_NAME", DataType::varying(30), Value::Char("Barbara".into()))
        .declare("OUT_NAME", DataType::varying(30), Value::Null)
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
them. A query, a searched UPDATE or a searched DELETE that finds one
row by a unique key, `WHERE K = ?` of a PRIMARY KEY or UNIQUE column K,
keeps its access path too, so its executions choose nothing. Another query keeps its access paths once
five executions in a row choose the same ones, as `WHERE K BETWEEN ? AND
?` does whose ranges are short, and is then planned no more, until a
table it reads gains or loses a tenth of its rows and 50 more, when it
looks again; one whose values keep choosing other paths is planned at
each execution after twenty. The plan is made again when the definitions change (a table
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
        .declare("TEXT", DataType::varying(200), Value::Char("INSERT INTO STAFF VALUES (?, ?)".into()))
        .declare("MY_ID", integer, Value::Exact(Decimal::new(1, 0)))
        .declare("MY_NAME", DataType::varying(30), Value::Char("Ada".into()));
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
            [exact @ DataType::Exact { .. }] => Ok(exact.clone()),
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

const FIVE: DataType = DataType::character(5);

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

### Indexes for function calls *(extension)*

A plug-in index can find rows for a WHERE filter written as a function
call. The example below registers `NEXT_TO(a, b)`, true when `a` is one
greater than `b`, and an index method that supports this query:

```sql
SELECT N FROM ITEMS WHERE NEXT_TO(N, ?);
```

With the parameter set to 40, the example index returns rows containing
40, 41 and 42. These are candidates: the database fetches them and checks
the full WHERE condition. Only 41 passes. This permits an index to return
extra rows while requiring it to find every true match. For example, a
spatial index could find shapes by their bounding boxes and leave the
exact function to reject the extra matches (RULES.md D34).

The example defines its own integer function and index. An application
must register both before using them; `NEXT_TO` is not a built-in
function. The built-in `btree` and `hash` methods retain their comparison
lookups. A plug-in supplies the search for a function.

The function and index opt in separately:

| API | What the implementation promises |
|---|---|
| `ScalarFunction::index_predicate` | True means every call accepted by the function's type checks returns BOOLEAN or null without an error or side effect. Equal inputs give equal answers, and either argument being null gives null. `deterministic` must also return true. |
| `IndexMethod::supports_predicate` | True means every index it creates on the given key types returns every match for the registered function, indexed column and argument position. Support covers every accepted value of the other argument, including null. |
| `Index::search` with `Probe::Predicate` | Return matching inserted `(Key, RowId)` pairs, each once. Extra candidates are allowed. Return the original key values so the executor can check the visible row version. |

Both opt-ins default to false and their promises hold for the lifetime
of the registered implementations. The planner recognizes calls with
two arguments: one indexed column and one constant or parameter, in
either order. `Predicate::column` is the column's position in the index
key; `Predicate::argument` is its position in the call, 0 or 1.
`Probe::Predicate::value` supplies the other argument. The example method
below supports the column in the first argument only.

The planner compares the lookup cost with a scan. `Index::estimate`
counts every candidate, including those the WHERE check rejects. If it
returns `None`, the planner prices fetching every table row. If
`probe_pages` returns `None`, the planner uses its usual estimate of a
tree descent. Each prepared execution supplies its current parameter to
both the lookup and the WHERE check, including when it reuses a learned
plan (D45). EXPLAIN shows the function lookup and the filter above it.

The lookup must preserve results and errors:

- If the function, an argument expression or another part of the WHERE
  filter can raise, the table is read whole. Skipping a row must not hide
  an error (D15).
- Every candidate is fetched and checked against the whole WHERE
  condition. FALSE and unknown discard the row. This lookup never uses
  an index-only scan.
- An index of collation sort keys cannot supply the function's original
  column values, so it is not eligible for a function lookup.
- If another key column may be null, the index must hold null keys to
  avoid missing matches. The planner keeps the requested sort when those
  nulls could survive the filter (D34, D66).
- The executor records a read of the whole table, including nulls, so
  conflict checks catch rows inserted or changed by another transaction.
  Storage checks never call the function while holding their locks
  (D29, R-ERR-4).

The traits are `Send + Sync`. Implementations protect their own state
when sessions call them concurrently and never call back into a database
or session (D27, D42, R-ERR-4).

This complete program wraps a B-link tree to return the three integer
candidates described above. Its estimate of three candidates makes the
planner choose the lookup over scanning the 201 rows. It also executes
the prepared statement with a null argument and checks that the plan
contains both `IndexScan` and the WHERE filter.

```rust
use std::sync::Arc;
use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};
use turborel::functions::ScalarFunction;
use turborel::index::{BTreeMethod, Entries, Index, IndexMethod, Key, KeyOrder, Predicate, Probe};
use turborel::params::{ParameterType, Params};
use turborel::storage::RowId;
use turborel::value::{CompOp, DataType, Decimal, Spelled, Value};

const FUNCTION: &str = "NEXT_TO";
const METHOD: &str = "next_candidates";
const INTEGER: DataType = DataType::Exact {
    precision: 10,
    scale: 0,
    spelled: Spelled::Integer,
};
const CANDIDATES: usize = 3;
const EXTRA: i128 = 2;
const ROWS: i128 = 200;

fn integer(t: &DataType) -> bool {
    matches!(t, DataType::Exact { scale: 0, .. })
}

struct NextTo;
impl ScalarFunction for NextTo {
    fn name(&self) -> &str {
        FUNCTION
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.len() != 2 || !args.iter().all(integer) {
            return Err(SqlError::new(
                Code::SyntaxRule,
                "[EXTENSION] NEXT_TO takes two integers",
            ));
        }
        Ok(DataType::Boolean)
    }
    fn parameter_type(
        &self,
        _position: usize,
        _args: &[Option<DataType>],
    ) -> Option<ParameterType> {
        Some(ParameterType::Of(INTEGER))
    }
    fn index_predicate(&self) -> bool {
        true
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        let [Value::Exact(a), Value::Exact(b)] = args else {
            return Err(SqlError::new(
                Code::Api,
                "[DESIGN] NEXT_TO requires two integer values",
            ));
        };
        Ok(Value::Bool(a.units.checked_sub(b.units) == Some(1)))
    }
}

struct CandidateMethod;
impl IndexMethod for CandidateMethod {
    fn name(&self) -> &'static str {
        METHOD
    }
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn supports_predicate(&self, p: &Predicate, columns: &[DataType]) -> bool {
        let one_integer = columns.len() == 1 && columns.first().is_some_and(integer);
        p.function.0.name() == FUNCTION && p.column == 0 && p.argument == 0 && one_integer
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        Ok(Box::new(CandidateIndex(Arc::from(
            BTreeMethod::in_memory().create(columns)?,
        ))))
    }
}

struct CandidateIndex(Arc<dyn Index>);
impl Index for CandidateIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.0.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.0.remove(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.0.rows(key)
    }
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        Some(CANDIDATES)
    }
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        Some(1.0)
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        let Probe::Predicate { value, .. } = probe else {
            return Arc::clone(&self.0).search(probe, order);
        };
        let Value::Exact(n) = value else {
            return Box::new(std::iter::empty());
        };
        // D34: include n and n + 2 so the WHERE check must reject them.
        let high = Value::Exact(Decimal::new(n.units.saturating_add(EXTRA), 0));
        let candidates = Probe::Range {
            equal: Vec::new(),
            low: Box::new((CompOp::Ge, value.clone())),
            high: Box::new((CompOp::Le, high)),
        };
        Arc::clone(&self.0).search(&candidates, order)
    }
}

fn main() -> Result<()> {
    let db = Database::new();
    db.register_scalar_function(Box::new(NextTo))?;
    db.register_index_method(Box::new(CandidateMethod))?;
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE ITEMS (N INTEGER)")?;
    let ann = db.session("ANN");
    ann.run("SET SQL.FUNCTION_CALLS = 'ON'")?;
    for n in 0..ROWS {
        ann.run(&format!("INSERT INTO ITEMS VALUES ({n})"))?;
    }
    ann.run("INSERT INTO ITEMS VALUES (NULL)")?;
    ann.commit()?;
    ann.run("CREATE INDEX NEAR_ITEMS ON ITEMS (N) USING next_candidates")?;
    let query = ann.prepare("SELECT N FROM ITEMS WHERE NEXT_TO(N, ?)")?;
    let mut rows = ann.open_prepared(&query, &[Value::Exact(Decimal::new(40, 0))])?;
    while let Some(row) = rows.fetch()? {
        if let Some(value) = row.first() {
            println!("{value}");
        }
    }
    let mut unknown = ann.open_prepared(&query, &[Value::Null])?;
    println!(
        "null argument: {} rows",
        usize::from(unknown.fetch()?.is_some())
    );
    let shown = ann.physical_plan("SELECT N FROM ITEMS WHERE NEXT_TO(N, 40)", &Params::new())?;
    println!("predicate index: {}", shown.contains("IndexScan"));
    println!("recheck: {}", shown.contains("Filter NEXT_TO"));
    Ok(())
}
```

Output:

```text
41
null argument: 0 rows
predicate index: true
recheck: true
```

## Spatial storage primitives

`spatial::Bounds` represents a finite, closed Cartesian rectangle in X/Y
order. It accepts points and zero-width rectangles, rejects reversed bounds
and non-finite coordinates, and treats touching boundaries as intersections.

```rust
use turborel::spatial::Bounds;

fn main() -> Result<(), turborel::error::SqlError> {
    let window = Bounds::new([0.0, 0.0], [10.0, 10.0])?;
    let point = Bounds::point([10.0, 5.0])?;
    assert!(window.intersects(point));
    assert!(window.contains(point));
    Ok(())
}
```

Storage implementations can use `engine::rtree::RTree` with a registered
buffer-pool file. It inserts and removes counted `(Bounds, RowId)` entries
and streams intersecting entries once each. Clones share its node locks;
readers hold a page latch only while copying that page. Its meta page is
the locator used to reopen it after the owning pager is checkpointed.

`RTree::nearest` streams entries by Euclidean rectangle distance, breaking
ties by row identifier. Take the first k entries to stop as soon as those
neighbours are known. Its priority queue reserves memory from the supplied
`MemoryBudget` and returns SQLCODE -918 if it cannot grow. Dropping the
iterator releases that memory. Distances use the coordinates' units.

These are rectangle storage operations. An exact geometry predicate needs
to test the candidate geometry, and rectangle containment has different
boundary semantics from a geometry's topological ST_Contains operation.

## See also

[The Client Library](02-library.md), to connect to a server instead;
[Statements](../sql/statements/README.md); the crate's documentation,
`cargo doc -p turborel`.
