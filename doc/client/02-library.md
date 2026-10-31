# The Client Library

[Contents](README.md)

The crate `turborel-client` connects a Rust program to a turborel
server, over the native protocol: blocking, on the standard library
alone; over TLS with its feature `tls`, trusting the platform's
certificate authorities too with `system-roots`; and proving a password by
SCRAM-SHA-256 with its feature `scram`. A program that embeds the
database uses the crate `turborel` instead, and its `Database` and
`Session` ([The Embedded Library](03-embedding.md)).

## Connecting

`connect` connects over TCP, `connect_local` over a Unix socket (on
Windows, a named pipe, as `\\.\pipe\turborel`), and
`target::connect_to` to a `Target`, which may be either, over TLS or
not. Each takes the user to connect as and a `Mechanism` that proves it:

- `scram::Scram`, of a password, by SCRAM-SHA-256, with the feature
  `scram`: what a password should be proved by. The password never
  travels; the connection is refused (SQLSTATE 08006) unless the server
  proves it knows the user's verifier. Its exchange overheard lets the
  password be guessed at leisure, so it is used only over TLS, a Unix
  socket or a named pipe, unless `Scram::in_the_clear` says otherwise,
  for a server whose SERVER.SCRAM_WITHOUT_TLS is ON.
- `Password`, of a password, sent whole, only over TLS, a Unix socket or
  a named pipe.
- `Certificate`, which proves the user by the certificate presented
  over TLS: `tls::presenting` makes a TLS that trusts the PEM file of
  authorities given and presents the certificate chain and key of two
  more.
- `Trust`, which asks the server to trust the user as it names itself,
  as a server does only of a client of its own machine, and only if its
  setting SERVER.TRUST is LOCAL, not its default.

To a server over plain TCP, `connect` and `connect_to` refuse
`Password`, and `Scram`, before they connect (SQLSTATE 28000); of a
stream given to `Connection::open` the caller answers for its privacy.
A client's TLS trusts the authorities of the files it is given alone,
unless the platform's are asked for: `tls::roots` takes those of a file,
the platform's, or both, for `tls::config` or `tls::presenting_to`; the
platform's only with the feature `system-roots`, which takes the crate
rustls-native-certs. They are the store's of the platform, or those of
the files SSL_CERT_FILE and SSL_CERT_DIR name instead, if either is set.

```rust
use turborel_client::scram::Scram;
use turborel_client::tls;

let config = tls::trusting(Path::new("/etc/turborel/ca.pem"))?;
let conn = tls::connect("db.example:7422", ("db.example", &config), "ANN", &mut Scram::new("ann's"))?;
```

```rust
use turborel_client::connection::{connect, Trust};

let mut conn = connect("127.0.0.1:7422", "ANN", &mut Trust)?;
println!("{}, as {}", conn.server(), conn.authorization());
// turborel 0.0.0 (alpha), as ANN
```

A connection is a session of the server's. `terminate` ends it, its
transaction as the setting SESSION_END says. A commit that fails then,
as one of a deferred constraint not satisfied (SQLSTATE 40002), rolls
the transaction back, and `terminate` gives its `Failure`. It waits for
the server to close at most as the connection's read timeout says, else
`TERMINATE_WITHIN`, 60 seconds.

## Timeouts

`timeouts::Timeouts` says how long a connection waits on its server:
`connect`, to connect, begin TLS, and for each step of the login, 30
seconds by default (`CONNECT_WITHIN`); `read`, for each read or write of
a request or reply after, no limit by default, as a statement may run
as long as it must. `connect`, `connect_local`, `tls::connect` and
`connect_to` wait as the default says; `target::connect_to_within` takes
`Timeouts` of its own, and `set_read_timeout` changes a connection's
read timeout. A wait that runs out loses the connection (SQLSTATE
08006), saying the server did not answer in time. A connection over a
stream given to `Connection::open` waits as its maker set the stream to.
A named pipe, on Windows, is bounded only as it is opened, while it is
busy: its reads and writes, of the login too, wait as long as they must,
as the standard library sets no limit on them.

```rust
use std::time::Duration;
use turborel_client::target::{Target, connect_to_within};
use turborel_client::timeouts::Timeouts;

let within = Timeouts { connect: Some(Duration::from_secs(5)), read: Some(Duration::from_secs(60)) };
let mut conn = connect_to_within(&Target::Tcp("127.0.0.1:7422".parse()?), "ANN", &mut Trust, within)?;
```

A reply is taken up to `MAX_BODY`, 16 MiB, the protocol's longest;
`set_max_body` takes replies of at most as many bytes as it says, a
longer one losing the connection.

## Running statements

`query` runs one statement, and gives its outcome: a query's columns
and rows, `Outcome::Rows`, or another statement's completion,
`Outcome::Done`, of its SQLCODE and SQLSTATE. A statement that fails
gives a `Failure`, of its SQLCODE, SQLSTATE, message, the rule it
cites apart, and its position in the statement. `query_into` gives a
query's rows to a `Sink` a batch at a time, as they come, rather than
all at once.

```rust
match conn.query("SELECT ID, NAME FROM STAFF WHERE ID < 3 ORDER BY ID")? {
    Outcome::Rows { columns, rows } => {
        // columns ID and NAME; rows [1, "Ada"] and [2, "Grace"]
    }
    Outcome::Done(done) => { /* done.sqlcode, done.sqlstate */ }
}
match conn.query("SELECT * FROM NOPE") {
    Err(f) => { /* f.sqlcode -103, f.sqlstate "2A000",
                   f.message "table ANN.NOPE is not defined" */ }
    Ok(_) => {}
}
```

A value is a `turborel::value::Value`: `Exact` of a `Decimal`, `Approx`,
`Char`, `Bits`, `Datetime`, `Interval`, `Extension` of a plug-in's
type, or `Null`. `in_transaction` says whether the session's
transaction is active, as the server said after the last statement. `canceller` gives what cancels the statement running
from another thread: `cancel` gives up after `CANCEL_WITHIN`, 10 seconds,
if the server does not answer; `cancel_within` takes a limit of its own.

## Prepared statements

`prepare` prepares a statement once, its dynamic parameters written
`?`; `execute` runs it with values for them, as many times as wanted.
`parameters` gives the type each `?` takes, `columns` a query's columns.
`open_cursor` runs a prepared query and gives its first rows, `fetch`
the next batch, `close` ends it; `deallocate` lets the statement go.

```rust
let by_id = conn.prepare("SELECT NAME FROM STAFF WHERE ID = ?")?;
// by_id.parameters(): [INTEGER], ID's type
if let Outcome::Rows { rows, .. } = conn.execute(&by_id, &[Value::Exact(Decimal::new(2, 0))])? {
    // rows: [["Grace"]]
}
```

A prepared statement stays valid across transactions. A query, an
INSERT, and a searched UPDATE or DELETE keep their plan between
executions, each execution putting its values in it; the plan is made
again when the definitions, a setting, or the session's schema, user or
time zone changed since, and at each execution if it holds the
statement's instant, as [The Embedded Library](03-embedding.md#prepared-statements)
tells.

## Importing

`import::import` inserts the rows of a `Source` into a table over a
connection, as the command-line client's `--import` does ([The
turborel Command](01-turborel.md#importing)). A `Source` is any type
that gives `headings`, its fields' names, and types if it knows them,
or none; then each `record`, a value for each field and where it
began, its record number and line, until it gives none. It holds a
record at a time, so a source of any size is read in bounded memory.
`Delimited` reads CSV (`Dialect::csv`) and tab-separated text
(`Dialect::tsv`) from any `BufRead`, with a header, a delimiter and a
text for null as its `Dialect` says.

`Import` names the table, how many rows are committed at a time
(`batch`, `DEFAULT_BATCH` 1000, 0 for all at once), and whether a row
that fails is skipped (`skip`), each given to the closure passed. The
connection's session must have no transaction active. A row that
fails ends the import with an `ImportError`: where its record began,
why (`Cause::Input`, the source's, or `Cause::Sql`, a `Failure`), how
many rows were committed and how many of the batch rolled back.

```rust
use std::fs::File;
use std::io::BufReader;
use turborel_client::import::{Delimited, Dialect, Import, import};

let file = BufReader::new(File::open("staff.csv")?);
let mut source = Delimited::new(file, Dialect::csv().with_header());
let done = import(&mut conn, &mut source, &Import::into("STAFF"), &mut |skipped| {
    eprintln!("{skipped}");
})?;
// done.rows: 2
```

## The SQL-agent

`agent::Agent` holds a program's connections, each named, one current,
and runs [CONNECT](../sql/statements/connect.md), [SET
CONNECTION](../sql/statements/set-connection.md) and
[DISCONNECT](../sql/statements/disconnect.md) itself, as SQL-92's
SQL-agent does; every other statement goes to the current connection.
A `Connector` the program gives makes the connections CONNECT asks for,
to the address it names, as the user it names. The command-line client
is built on it.
