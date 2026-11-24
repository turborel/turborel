# The Client Library

[Contents](README.md)

The crate `turborel-client` connects a Rust program to a turborel
server, over the native protocol: blocking, on the standard library
alone; over TLS with its feature `tls`, trusting the platform's
certificate authorities too with `system-roots`; and proving a password by
SCRAM-SHA-256, or SCRAM-SHA-256-PLUS over TLS, with its feature `scram`. A program that embeds the
database uses the crate `turborel` instead, and its `Database` and
`Session` ([The Embedded Library](03-embedding.md)). Settings read from
outside the program, several servers, TLS by mode, pipelines, pools and
the SCRAM key cache are in [Settings, Pipelines and
Pools](02-library-connections.md).

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
  for a server whose SERVER.SCRAM_WITHOUT_TLS is ON. Over TLS, by
  `tls::connect` or `connect_to`, it is SCRAM-SHA-256-PLUS, bound to the
  connection, over TLS 1.3 by `tls-exporter`, over TLS 1.2 by the
  certificate the server presented, so a server in the middle cannot
  relay it; a program that opens a TLS stream of its own and gives it to
  `Connection::open` binds it first, by `Mechanism::bind` with
  `tls::channel`.
- `Password`, of a password, sent whole, only over TLS, a Unix socket or
  a named pipe.
- `Certificate`, which proves the user by the certificate presented
  over TLS: `tls::presenting` makes a TLS that trusts the PEM file of
  authorities given and presents the certificate chain and key of two
  more.
- `Trust`, which asks the server to trust the user as it names itself,
  as a server does only of a client of its own machine, and only if its
  setting SERVER.TRUST_LOCAL is ON, not its default.

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
transaction as the setting SESSION.COMMIT_AT_END says. A commit that fails then,
as one of a deferred constraint not satisfied (SQLSTATE 40002), rolls
the transaction back, and `terminate` gives its `Failure`. A Notice or
a Status the server sends before that Error is read past. On a stream
that bounds itself it waits for the server to close at most as the
connection's read timeout says, else `TERMINATE_WITHIN`, 60 seconds.
Every connection the library makes is over such a stream, but a named
pipe on Windows, and so is one `Connection::open_within` makes. A
stream given to `Connection::open` waits as its maker set it.

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
stream given to `Connection::open` waits as its maker set the stream to,
`terminate` too. `Connection::open_within` takes a stream of any type
that implements `timeouts::Bound`, a program's own too, and bounds it as
its `Timeouts` say, `terminate` as above, and `set_read_timeout` then
works on it as on a connection the library makes.
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

A reply whose frame is whole but holds a value the client cannot take,
as damaged JSON or a truth byte other than 0 or 1, fails its statement
with that value's error, SQLCODE -916 or another not of class 08. The
connection goes on. The rest of the statement's replies are read past
first, so the next statement gets its own. A frame that is itself
malformed, cut short or with bytes after its last field, loses the
connection (SQLSTATE 08006).

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
    Err(f) => { /* f.sqlcode -103, f.sqlstate "42000",
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

A transaction begins with the first statement and lasts until COMMIT or
ROLLBACK, as SQL-92 has it. `set_autocommit(true)` makes each statement
`query`, `query_into` or `execute` runs its own transaction instead: the
server commits it as the statement succeeds, or rolls it back as it
fails, within the same request, so no COMMIT is sent. A cursor
`open_cursor` opens is read within its transaction either way.

## Prepared statements

`prepare` prepares a statement once, its dynamic parameters written
`?`; `execute` runs it with values for them, as many times as wanted.
`inputs` gives the type each `?` takes that `execute` gives a value
for, `columns` a query's columns. `open_cursor` runs a prepared query
and gives its first rows, `fetch` the next batch, `close` ends it;
`deallocate` lets the statement go.

```rust
let by_id = conn.prepare("SELECT NAME FROM STAFF WHERE ID = ?")?;
// by_id.inputs(): [INTEGER], ID's type
if let Outcome::Rows { rows, .. } = conn.execute(&by_id, &[Value::Exact(Decimal::new(2, 0))])? {
    // rows: [["Grace"]]
}
```

`parameters` gives every `?`, each a `DynamicParameter` of its type, its
mode and its ordinal position. A `?` of a CALL has the mode and the
ordinal position of the procedure's parameter whose argument holds it,
IN, OUT or INOUT, and `procedure` gives the procedure's specific name.
`execute` takes values for the IN and INOUT ones and gives back the OUT
and INOUT ones' values as one row of the statement's `columns`. A `?`
of any other statement is IN, of no position. The server tells the
modes to a connection granted `parameter-modes`, which `connect` asks
for. Without it, `parameters` holds the inputs alone, each IN.

```rust
// CREATE PROCEDURE MIXED (OUT TOTAL INTEGER, IN TIMES INTEGER, INOUT TAG VARCHAR(10)) ...
let mixed = conn.prepare("CALL MIXED(?, ?, ?)")?;
// mixed.parameters(): OUT of 1, IN of 2, INOUT of 3
// mixed.inputs(): [INTEGER, VARCHAR(10)]
if let Outcome::Rows { rows, .. } = conn.execute(&mixed, &[Value::Exact(Decimal::new(2, 0)), Value::Char("A".into())])? {
    // rows: [[TOTAL, TAG]], the OUT and INOUT parameters' values
}
```

A `Column`, of a statement's `columns` or of `Outcome::Rows`, has its
name, its `data_type` and whether it is NOT NULL. A type's tag can stand
for several declared types, so an INTEGER column's `data_type` is an
exact numeric type of precision 10 and scale 0. Granted
`declared-types`, which `connect` asks for, its `declared` holds the
type it was declared with, its name as the Information Schema gives it
and the type as SQL writes it. `type_written` gives the declared type
where the server told it, else the type its tag says.

```rust
let typed = conn.prepare("VALUES (CAST(1 AS INTEGER), CAST(2.5 AS DECIMAL(9,2)))")?;
// typed.columns()[0].type_written(): "INTEGER"
// typed.columns()[1].type_written(): "DECIMAL(9,2)"
// without declared-types granted, "NUMERIC(10,0)" and "NUMERIC(9,2)"
```

A prepared statement stays valid across transactions. A query, an
INSERT, and a searched or positioned UPDATE or DELETE keep their plan
between executions, each execution putting its values in it; the plan
is made again when the definitions, a setting, or the session's schema,
user or time zone changed since, and at each execution if it holds the
statement's instant, as [The Embedded Library](03-embedding.md#prepared-statements)
tells.

## Large objects

A CLOB or BLOB value of at most SERVER.LOB_INLINE_BYTES, 64 KiB unless
the operator sets otherwise, comes in its row whole. A longer one comes
as a `Value::Lob` that holds its locator and its lengths, not its
octets. `read_lob` reads a range of it, by octets or by a CLOB's
characters, at most a chunk of 256 KiB at a time. `lob_reader` gives a
`std::io::Read` that reads it whole, a chunk at a time.

A locator is valid until the transaction that read it ends. A statement
whose reply sent one keeps its transaction open, under `set_autocommit`
too, so commit once the value is read. `free_lob` ends the locator
sooner, and `hold_lob` keeps it past the commit, until it is freed or a
rollback ends it.

`create_lob` writes a value a piece at a time, through a
`std::io::Write`, and `finish` gives it as a `Value::Lob` to pass as a
parameter of a CLOB or BLOB. A CLOB's octets must be UTF-8, and a
character may be split between writes.

```rust
use std::io::{Read, Write};
use turborel::value::lob::LobKind;

let mut writer = conn.create_lob(LobKind::Binary)?;
for chunk in std::iter::repeat_n([7_u8; 4096], 1024) {
    writer.write_all(&chunk)?;
}
let scan = writer.finish()?;
let insert = conn.prepare("INSERT INTO SCANS VALUES (1, ?)")?;
conn.execute(&insert, &[Value::Lob(scan)])?;
conn.query("COMMIT")?;
if let Outcome::Rows { rows, .. } = conn.query("SELECT IMAGE FROM SCANS")? {
    if let Value::Lob(image) = &rows[0][0] {
        let mut octets = Vec::new();
        conn.lob_reader(image).read_to_end(&mut octets)?;
        // octets.len(): 4194304
    }
}
conn.query("COMMIT")?;
```

An import sends a field longer than 64 KiB for a CLOB or BLOB column
by `create_lob`, so no message carries more. A field for a BLOB, BINARY
or VARBINARY column is read as its octets' hexits, as an export writes
them ([Copying and exporting](#copying-and-exporting)).

## XML values

An XML value comes as a `Value::Xml`, its serialization: `text` gives it,
and `sequence` parses it for its nodes. To send one as a parameter of an
XML type, make it of its text with `XmlValue::serialized`. A value the
server cannot serialize, such as an attribute standing alone, fails the
fetch with SQLSTATE 2200W.

```rust
use turborel::value::xml::XmlValue;

let insert = conn.prepare("INSERT INTO ORDERS VALUES (?, ?)")?;
let order = XmlValue::serialized("<order no=\"18\"><item>jam</item></order>");
conn.execute(&insert, &[Value::Exact(Decimal::new(18, 0)), Value::Xml(order)])?;
if let Outcome::Rows { rows, .. } = conn.query("SELECT BODY FROM ORDERS WHERE ID = 18")? {
    if let Value::Xml(body) = &rows[0][0] {
        let text = body.text()?;
        // text: <order no="18"><item>jam</item></order>
    }
}
```

## UUIDs

A [UUID](../sql/02-data-types.md#uuids) comes as a `Value::Extension` of
the UUID type, whatever types the client has registered.
`turborel::value::types::uuid::octets_of` gives its 16 octets and
`uuid::text` its text. To send one as a parameter, make it of its octets
with `uuid_value`, or of its text with `uuid::parse`.

```rust
use turborel::value::types::uuid::{self, octets_of, uuid_value};

let insert = conn.prepare("INSERT INTO TICKETS VALUES (?, ?)")?;
let id = uuid::parse("919108f7-52d1-4320-9bac-f847db4148a8").unwrap_or_default();
conn.execute(&insert, &[uuid_value(id), Value::Char("12A".into())])?;
if let Outcome::Rows { rows, .. } = conn.query("SELECT ID FROM TICKETS")? {
    if let Some(octets) = octets_of(&rows[0][0]) {
        let text = uuid::text(&octets);
        // text: 919108f7-52d1-4320-9bac-f847db4148a8
    }
}
```

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

A field for a binary string column, a BLOB's among them, is read as the
octets its hexits write, two to each, of either case, so what an export
wrote reads back. One that holds anything else, or an odd number of
hexits, fails its row with SQLSTATE 22018 (SQLCODE -301).

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

## What a connection asks for

A connection asks the server for capabilities as it opens, and the server
grants those it serves ([The Native Protocol](05-protocol.md)). `connect`
asks for every one but compression. `Connection::open_offering` takes an
`Offer` of its own: `Offer::none()` asks for none, `compressed()` adds
compression, and `attribute(name, value)` adds a connection attribute,
which the server shows in INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES, the
one named `application` in CONNECTIONS too. `granted` gives what was
granted. A method that needs a capability not granted fails before it
sends anything.

## What the server tells

Granted `status`, `setting(name)` gives the value of one of the
session's settings as the server last told it, with no round trip, and
`settings` all of them. The server tells them as the session begins, and
again as a statement changes one, so a SET is seen at once. Besides the
settings, AUTHORIZATION, SCHEMA, READ_ONLY and SERVER_VERSION are told.

A statement's warnings come as notices before its outcome. A handler set
by `set_notice_handler` is given each as it comes. With none, the last 64
are kept, and `take_notices` takes them. Granted `conditions`, the
warnings come in the completion instead, and `conditions` gives the last
statement's, each a `Failure` of its SQLSTATE, SQLCODE and message.

The last statement's completion tells more, each part as granted.
`rows_affected` gives the rows an INSERT, UPDATE, DELETE or MERGE
changed, as GET DIAGNOSTICS ROW_COUNT gives them, or the rows a query
returned. `last_generated` gives the identity values an INSERT
generated, in the order of its rows, as SQL writes them; none of a
trigger's statements, and none of a MERGE. `server_time` gives the
server's time for the statement in microseconds, from when a worker
began it until its completion was ready, a cursor's summed over its
fetches, with the time its request waited for a worker and the time it
waited for locks. `commit_position` gives the log position of the last
commit, for a replica's session to read after
([Replication](../operator/09-replication.md)).

```rust
conn.set_autocommit(true);
conn.query("INSERT INTO STAFF (NAME) VALUES ('Edsger'), ('Barbara')")?;
// conn.rows_affected(): Some(2)
// conn.last_generated(): ["3", "4"], STAFF's identity column ID
// conn.server_time(): Some(Timing { total: 180, queued: 4, locks: 0 })
```

A failure granted `diagnostics` carries what it is about, its
`diagnostics` giving its severity, detail, hint, context, and the
schema, table, column, data type and constraint, each if known. A
duplicate key names its constraint and table, and its detail gives the
key.

Under autocommit, a cursor `open_cursor` opens ends its transaction as
it ends when `fetch-ends` is granted. The fetch that reads its last row
commits, a fetch that fails rolls back, and `close` commits.

## Controlling the session

`ping` asks whether the server and the connection are alive, with no
statement run. `reset` puts the session back as it was at login, its
transaction rolled back, its prepared statements and settings gone, its
user kept. `change_user` authenticates the connection again as another
user, whose new session it then is. A transaction either rolls back is
told by notice 01T01.

`set_query_attributes` sets attributes every later statement carries,
read in SQL by QUERY_ATTRIBUTE(name), until set again. One named
`traceparent` is written in the server's log of a slow statement.

`query_many` runs a text of several statements as one request, each in
order, until one fails. Its `Ran` gives each outcome, and the failure
that stopped the rest.

`describe` describes a prepared statement again. `open_cursor_numbered`,
`fetch_numbered` and `close_numbered` keep several cursors of one
statement open at once, each by a number of the program's, and
`describe_cursor` gives an open cursor's columns.

## Debugging routines

*(extension)* With `debugger` granted, one connection drives the
routines another connection's session runs
([Debugging Routines](../user/11-debugging.md)). `debug_id` gives a
connection's ID, and `debug_attach` attaches another connection to its
session. `debug_break` sets a breakpoint at a line of a procedure's, a
function's or a trigger's text, `debug_clear` clears one and
`debug_breakpoints` lists them. `debug_wait` gives the next event of
the target, a pause with its call stack, a timeout, a cancel or its
end, waiting at most the time it is given. `debug_stack` gives the
pause again, `debug_resume` lets the paused session go on, a step into,
over or out, or to a breakpoint, and `debug_interrupt` pauses it at its
next statement. `debug_detach` ends the debugging.

```rust
use turborel_client::debug::{Action, Event, Kind};
let mut target = connect("127.0.0.1:7422", "ANN", &mut Trust)?;
let mut debugger = connect("127.0.0.1:7422", "ANN", &mut Trust)?;
debugger.debug_attach(target.debug_id()?)?;
let routine = QualifiedName { auth: String::new(), ident: "RAISE_PAY".into() };
debugger.debug_break((Kind::Procedure, &routine), 4, None)?;
// The target's CALL RAISE_PAY(10) runs on another thread.
if let Some(Event::Paused(paused)) = debugger.debug_wait(Some(Duration::from_secs(10)))? {
    let top = paused.frames.last();
    debugger.debug_resume(Action::Continue)?;
}
```

## Copying and exporting

`copy_in` begins a copy of rows into a table, of the columns named, or
all. `Copying::send` sends rows, many to a message and no reply each, and
`finish` ends the copy, giving the rows inserted. Each value is assigned
to its column as INSERT assigns it. A copy is all or nothing, so a row
refused undoes every row of it, its failure's detail naming the row. A
copy dropped before `finish` is abandoned. `copy_rows` copies the rows
an iterator gives. Importing copies where the server grants it, and no
column is a large object.

```rust
let rows = vec![vec![Value::Exact(Decimal::new(7, 0)), Value::Char("Niklaus".into())]];
let inserted = conn.copy_rows("STAFF", &["ID", "NAME"], rows)?;
// inserted: 1
```

`export` runs a query and writes its rows to any `Write` as they come,
as CSV, a header of its columns' names then a record for each row
(`ExportFormat::Csv`), or as the protocol's own frames
(`ExportFormat::Native`), which `export::read_native` reads back with
every value as it was. `export_with` writes them by any `export::Layout`,
the program's own among them. `export::Delimited` writes CSV, or
tab-separated text with `Delimited::text`, its header, delimiter, null
text and line end as its fields say, and `export::Native` the frames.

A large object is written as its value, never its locator. A CLOB or an
NCLOB is written as its text. A BLOB, as every binary string, is written
as its octets in hexits, two to each, upper case, as between the quotes
of `X'...'`, which an import reads back into a binary string column. One
the server sends by its locator, longer than 64 KiB, is read a chunk at
a time as it is written, its locator freed once read, so no more than a
chunk of it is held. In CSV it is quoted whatever it holds. In native
rows its octets follow the Rows frame that holds it, in Lob frames of a
chunk each. A large object within a row, an array or a structured value
is read whole, as its value's text needs it all.

The rows of a query whose columns may hold a large object come through a
cursor, 256 at a time, its large objects read between the fetches, as
the transaction they were sent in lasts. Under autocommit, a transaction
the export began is committed at its end, or rolled back if it failed.
A query whose columns hold none comes in one request, its rows written
as they arrive.

```rust
use turborel_client::export::ExportFormat;

let mut csv = Vec::new();
let rows = conn.export("SELECT ID, PHOTO FROM BADGES ORDER BY ID", &mut csv, ExportFormat::Csv)?;
// rows: 2
// csv: ID,PHOTO\r\n1,CAFE\r\n2,\r\n
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
