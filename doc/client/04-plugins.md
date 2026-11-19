# Plug-ins

[Contents](README.md)

An external plug-in is a program of another party that the database
runs as processes of its own and speaks to over their standard input and
output, in plug-in protocol 1. Its manifest declares the objects it
brings, and `CREATE EXTENSION` makes them the database's, of nineteen
kinds: functions, aggregates, table functions, procedures, vector
metrics, embedding models, authentication mechanisms, backup targets,
partitioning strategies, full-text rankings, vector quantizers,
collations, system tables, XQuery functions, data types, storage
engines, index methods, credentials and sinks of events
([Kinds of Plug-in](04-plugins-kinds.md)). A plug-in may be written in
any language. One that crashes,
hangs or misbehaves fails its call and nothing else, as it shares no
memory with the database.

A program that embeds the database may instead register plug-ins of
Rust in its own process ([The Embedded Library](03-embedding.md)).
[Packages and Registries](04-plugins-packages.md) tells how a plug-in is
packed, signed, published and installed.

## Loading an extension

The operator chooses the directories plug-ins may be in, by the node's
setting PLUGINS.DIRECTORIES, given as the server starts
(`--set PLUGINS.DIRECTORIES=/srv/turborel/plugins`) or by the program
embedding the database before it opens it. SQL may not set it. A
package installed there is a directory `<NAME>/<version>/` that holds
the manifest, `turborel-plugin.manifest`, and the program.

```sql
CREATE EXTENSION EXAMPLE;
VALUES (TWICE(21));
```

```
OK

--------------
           42
(1 row)
```

Only the database's owner loads an extension, as its program runs with
the server's rights; any user calls its objects. `CREATE EXTENSION`,
`ALTER EXTENSION ... UPDATE` and `DROP EXTENSION` run within a
transaction, so ROLLBACK undoes them, and are logged with the manifest
read, so recovery and replicas make the extension again.

## The manifest

A manifest is a list of items, each ended by a semicolon, read by SQL's
own tokens, so names and data types are written as in SQL and a NAME is
folded to upper case as an identifier is.

```
<manifest item> ::= NAME <identifier> | VERSION <character string literal>
    | PROTOCOL 1
    | EXECUTABLE [ FOR <character string literal> ] <character string literal>
    | REQUIRES <identifier> <character string literal>
    | TURBOREL <character string literal>
    | UPGRADE FROM <character string literal> SCRIPT <character string literal>
    | FUNCTION <identifier> ( <data type>, ... ) RETURNS <data type>
          [ [ NOT ] DETERMINISTIC ] [ BATCH <unsigned integer> ]
    | AGGREGATE <identifier> ( <data type> ) RETURNS <data type> [ BATCH <unsigned integer> ]
    | TABLE FUNCTION <identifier> ( <data type>, ... )
          RETURNS TABLE ( <column name> <data type>, ... ) [ READS SQL DATA ]
    | PROCEDURE <identifier> ( <data type>, ... ) [ MODIFIES SQL DATA ]
    | METRIC <identifier> [ FORMATS ( <format>, ... ) ]
          [ PROPERTIES ( SYMMETRIC | NON_NEGATIVE | TRIANGLE, ... ) ] [ BATCH <unsigned integer> ]
    | EMBEDDING MODEL <identifier> DIMENSIONS <unsigned integer> [ ELEMENT <format> ]
          [ [ NOT ] DETERMINISTIC ] [ BATCH <unsigned integer> ]
    | AUTHENTICATOR <character string literal>
    | TARGET <character string literal>
    | PARTITION STRATEGY <identifier>
    | RANKING <identifier> | QUANTIZER <identifier>
    | COLLATION <identifier> VERSION <character string literal>
    | SYSTEM TABLE <identifier> ( <column name> <data type>, ... )
    | XQUERY FUNCTION <character string literal> ( <data type>, ... ) RETURNS <data type>
    | TYPE <identifier> [ ORDERED ]
    | STORAGE ENGINE <identifier> [ BATCH <unsigned integer> ]
    | INDEX METHOD <identifier> SUPPORTS ( EQUALITY | RANGE | NEAREST, ... ) [ BATCH <unsigned integer> ]
    | CREDENTIALS <identifier>
    | DIAGNOSTICS <identifier> [ BATCH <unsigned integer> ]
```

NAME, VERSION, PROTOCOL and an EXECUTABLE are required. EXECUTABLE
names the program, a path inside the directory; one written FOR a
platform (`linux`, `macos`, `windows`) is taken there before one written
for any. VERSION is a semantic version. REQUIRES, TURBOREL and UPGRADE
are a package's ([Packages and Registries](04-plugins-packages.md)).

BATCH is how many rows, candidates or texts one call carries: 1,024 for
a function or aggregate, 4,096 for a metric and 64 for a model if it is
not written. A function DETERMINISTIC may generate a column and be
folded; one NOT DETERMINISTIC, the default, may not. A table function
that READS SQL DATA names the queries it reads, which the statement runs
with its user's privileges and sends it the rows of. A procedure that
MODIFIES SQL DATA may run statements in its caller's transaction; one
that does not may only query. An XQuery function is named by one
literal, its namespace's URI in braces before its local name, as
`'{http://example.com/fn}twice'`. [Kinds of Plug-in](04-plugins-kinds.md)
tells what each of the later kinds declares.

## Protocol 1

Each message is a frame. It begins with its length in four bytes, which
counts what follows, then its type in one byte, then its body. Numbers
are little-endian, and a string or bytes are their length in four bytes,
then them.

| Type | Message | From | Body |
|---|---|---|---|
| 1 | HELLO | database | protocol u32, extension str, capabilities (count u32, strs), max_message_bytes u64, max_call_bytes u64, memory_bytes u64 |
| 2 | WELCOME | plug-in | protocol u32, name str, version str, capabilities, kinds (count u32, each u8) |
| 3 | CALL | database | call u64, kind u8, op u8, object str, payload |
| 4 | RESULT | plug-in | call u64, payload |
| 5 | ERROR | either | call u64, SQLSTATE (5 bytes), message str |
| 6 | CANCEL | database | call u64 |
| 7 | SHUTDOWN | database | nothing |
| 8 | SQL | plug-in | call u64, rows u8, text str, values |
| 9 | SQL_DONE | database | call u64, rows (count u32, each values) |

The database begins with HELLO; the program answers WELCOME, naming the
extension HELLO named and every kind its manifest declares. Then the
database sends a CALL at a time and waits for its RESULT or ERROR. While
a procedure's call runs, its program may send SQL, a query if rows is 1,
and is answered SQL_DONE or ERROR. CANCEL asks the program to end a call
early; SHUTDOWN, or the end of its input, to exit.

A value is a tag and its body: 0 null; 1 truth value, a byte; 2 exact
number, units i128 and scale u32; 3 approximate number, the bits of a
DOUBLE in u64; 4 character string, str; 5 binary string, bytes; 6 the
text form of a value of another type, as a datetime or JSON; 7 array and
8 row, values; 9 dense vector, dims u32 and as many DOUBLEs; 10 sparse
vector, dims u32, then count u32 pairs of index u32 and DOUBLE. Values
are counted, a count u32 before them.

| Kind | Operations: CALL payload → RESULT payload |
|---|---|
| 1 function | 1 call: rows → values, one a row |
| 2 aggregate | 1 step: state bytes, values → state bytes; 2 merge: state, state → state; 3 finish: state → value |
| 3 table function | 1 rows: values, the rows of each query read → rows; 2 reads: values, null where not known → strs |
| 4 procedure | 1 call: values → nothing |
| 5 metric | 1 distances: a vector, vectors (count u32) → values, a DOUBLE or null each |
| 6 embedding model | 1 embed: strs → values, a vector each |
| 7 authenticator | 1 begin: user, peer, local u8, secure u8 → exchange u64; 2 step: exchange, bytes → outcome u8 (0 a challenge's bytes, 1 accepted, 2 refused and why) |
| 8 backup target | 1 open: location → target u64; 2 begin: target, object → upload u64; 3 abandon, 4 write (bytes), 5 finish: target, upload; 6 read: target, object, offset u64, max u32 → bytes; 7 list: target, prefix → strs; 8 remove: target, object |
| 9 partition strategy | 1 check key: types; 2 bound: types, bound → bound; 3 overlaps; 4 contains; 5 route; 6 admits |
| 10 ranking | 1 weights; 2 bounds |
| 11 quantizer | 1 train |
| 12 collation | 1 keys |
| 13 system table | 1 rows |
| 14 XQuery function | 1 call |
| 15 type | 1 input; 2 receive |
| 16 storage engine | 1 create; 2 open; 3 scan; 4 get; 5 apply; 6 checkpoint; 7 drop |
| 17 index method | 1 create; 2 open; 3 write; 4 search; 5 rows; 6 nearest; 7 checkpoint; 8 drop |
| 18 credentials | 1 verifier |
| 19 sink of events | 1 events |

An aggregate's state is the program's own bytes, which the database
keeps, so a program holds nothing between calls; it starts empty. An
exchange, a target and an upload are the program's, so the database
keeps one process for each until it ends. The payloads of kinds 10 to
19 are in [Kinds of Plug-in](04-plugins-kinds.md), a section each.

## The plug-in SDK

A program written in Rust serves its objects through the plug-in SDK,
the module `plugin` of the crate `turborel-protocol`. It builds a
`Plugin` of its objects, each an implementation of its kind's trait
under the name its manifest declares, and hands it to `serve`, which
reads its standard input and writes its standard output until the
database shuts it down. The SDK answers HELLO, reads each CALL's batch
and calls the trait once for each of its rows, values or texts, writes
the RESULT, and turns an error into an ERROR. Its manifest and program:

```
NAME DOUBLING; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE 'doubling';
FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC BATCH 100;
```

```rust
use turborel::value::{Decimal, Value};
use turborel_plugin::{Answer, Cx, Plugin, Scalar, serve};

struct Twice;

impl Scalar for Twice {
    fn call(&mut self, _cx: &mut Cx<'_>, args: &[Value]) -> Answer<Value> {
        match args.first() {
            Some(Value::Exact(d)) => Ok(Value::Exact(d.checked_mul(Decimal::new(2, 0))?)),
            Some(_) | None => Ok(Value::Null),
        }
    }
}

fn main() {
    if serve(Plugin::new("1.0.0").scalar("TWICE", Twice)).is_err() {
        std::process::exit(1);
    }
}
```

An object's name is as the database writes it in a CALL, an identifier
in upper case unless delimited, and an XQuery function's expanded name
as written. A `PluginError` is a SQLSTATE and a message, and an error
of the database's own, as a decimal overflow, converts into one with its
SQLSTATE kept. Each call that may run long is given a `Cx`.
`cx.check()` fails with SQLSTATE 57014 once the database has sent
CANCEL for the call, `cx.query` and `cx.run` run SQL in the call's
transaction, and `cx.limits()` gives the limits HELLO gave. An answer
longer than those limits is an ERROR of 38T04 and is not sent. A thread
of the SDK reads the input while a call runs, so a CANCEL is heard.

`Harness` runs a `Plugin` in a test's own process, each call made as the
database makes it, its payload written and its answer read, so objects
are tested with no database and no process:

```rust
use turborel_plugin::{Answer, Collation, Harness, Plugin};

struct Lower;

impl Collation for Lower {
    fn sort_key(&mut self, text: &str, _pad: bool) -> Answer<Vec<u8>> {
        Ok(text.to_lowercase().into_bytes())
    }
}

let mut h = Harness::new(Plugin::new("1.0.0").collation("LOWER", Lower));
assert_eq!(h.sort_keys("LOWER", &["B", "a"], false).unwrap(), [b"b".to_vec(), b"a".to_vec()]);
```

The example in the source, `plugin/examples/plugin_example`, is
built on the SDK and serves an object of every kind, the tests' plug-in.
Each kind's trait follows, with the `Plugin` method that adds it.

### Functions, aggregates, table functions and procedures

`Scalar::call(cx, args)` gives the value of one row's arguments
(`Plugin::scalar`). `Aggregate` steps a state the database keeps, the
program's own bytes, empty at first, merges two and finishes one
(`step`, `merge`, `finish`; `Plugin::aggregate`). `TableFunction` gives
its rows of its arguments and the rows of the queries `reads` named
(`Plugin::table_function`). `Procedure::call` runs its statements by
`cx.run` (`Plugin::procedure`).

### Metrics and embedding models

`Metric::distance(query, candidate)` gives one distance, or none where
it has none; a CALL of many candidates is answered by a call each
(`Plugin::metric`). `Embedder::embed(text)` gives a text's vector
(`Plugin::embedder`).

### Authenticators

`Authenticator::begin(who)` starts an exchange with the user and peer
`Who` names, and `Exchange::step(bytes)` answers each message of the
client by a `Step`, a challenge's bytes, acceptance or refusal
(`Plugin::authenticator`). The SDK keeps the exchange by its number.

### Backup targets

`Target::open(location)` opens a `TargetStore`, which begins an `Upload`
of an object, written in pieces and finished, reads a range of one,
lists them by prefix and removes one (`Plugin::target`). The SDK keeps
each store and upload by its number.

### Partitioning strategies

`PartitionStrategy` checks a key's types and each bound, and says
whether two bounds overlap and whether a bound contains a key; `route`
and `admits` have defaults built of those (`Plugin::partition_strategy`).

### Rankings

`Ranking::weight(corpus, term, in_doc)` gives one (term, document)
weight and `bound(corpus, term)` a term's greatest, from the statistics
the database sends (`Plugin::ranking`). A batch of WEIGHTS is answered a
call each.

### Quantizers

`Quantizer::train(training)` turns a sample into a `Trained` codebook,
`k` centroids in each subspace of its widths, which the database then
uses alone (`Plugin::quantizer`).

### Collations

`Collation::sort_key(text, pad)` gives a string's sort key, compared as
bytes, `pad` asking that trailing spaces not count
(`Plugin::collation`).

### System tables

`SystemTable::rows(user, owner)` gives every row for that reader. The
SDK keeps them and gives the database a page at a time, asking the
table again only for the next read (`Plugin::system_table`).

### XQuery functions

`XQueryFunction::call(args)` gives the value of atomic arguments
(`Plugin::xquery_function`).

### Types

`Type::input(text)` and `receive(bytes)` make a value, a `Made` of its
key, binary form and text form (`Plugin::data_type`).

### Storage engines

`Engine` creates a table and gives its locator, opens one by it (an
`Opened`, whether it has it, the next id, its rows and the position it
is durable through), scans a page after an id, gets rows by id, applies
changes all or none giving what each row held, keeps a checkpoint's
position, and drops a table (`Plugin::engine`).

### Index methods

`IndexMethod` creates an index, opens one (whether it has it and
whether it is clean), makes a batch of `Write`s, searches a page of
entries in key order by a `Search` and a cursor, gives a key's rows,
gives the nearest entries of a `Near`, keeps a checkpoint, and drops an
index (`Plugin::index_method`).

### Credentials and sinks of events

`Credentials::verifier(user, password)` makes a verifier
(`Plugin::credentials`). `Diagnostics::events(events, dropped)` is told
a batch of `Told` events, each a level, a kind and a text, and how many
were dropped as the queue was full (`Plugin::diagnostics`).

## A plug-in by hand

A program in another language speaks the protocol itself. This one, in
Rust with no SDK, is a whole plug-in of one function, SHOUT, its
argument in upper case. Its manifest:

```
NAME SHOUTING; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE 'shouting';
FUNCTION SHOUT(VARCHAR(200)) RETURNS VARCHAR(200) DETERMINISTIC;
```

```rust
use std::io::{Read, Write};

/// The next frame's type and body, or none at the end of input.
fn read(input: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut length = [0u8; 4];
    input.read_exact(&mut length).ok()?;
    let mut body = vec![0u8; u32::from_le_bytes(length) as usize];
    input.read_exact(&mut body).ok()?;
    let (&kind, rest) = body.split_first()?;
    Some((kind, rest.to_vec()))
}

/// Writes a frame of type `kind`.
fn write(output: &mut impl Write, kind: u8, body: &[u8]) -> std::io::Result<()> {
    output.write_all(&(body.len() as u32 + 1).to_le_bytes())?;
    output.write_all(&[kind])?;
    output.write_all(body)?;
    output.flush()
}

fn u32_at(b: &[u8], at: &mut usize) -> u32 {
    let n = u32::from_le_bytes(b[*at..*at + 4].try_into().unwrap());
    *at += 4;
    n
}

fn str_at(b: &[u8], at: &mut usize) -> String {
    let n = u32_at(b, at) as usize;
    let s = String::from_utf8_lossy(&b[*at..*at + n]).into_owned();
    *at += n;
    s
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend((s.len() as u32).to_le_bytes());
    out.extend(s.as_bytes());
}

fn main() {
    let (mut input, mut output) = (std::io::stdin(), std::io::stdout());
    while let Some((kind, body)) = read(&mut input) {
        let mut at = 0;
        let mut out = Vec::new();
        match kind {
            1 => {
                // HELLO: answer WELCOME, of the name it gives, serving kind 1.
                u32_at(&body, &mut at);
                let name = str_at(&body, &mut at);
                out.extend(1u32.to_le_bytes());
                put_str(&mut out, &name);
                put_str(&mut out, "1.0.0");
                out.extend(0u32.to_le_bytes());
                out.extend([1, 0, 0, 0, 1]);
                write(&mut output, 2, &out).unwrap();
            }
            3 => {
                // CALL of SHOUT: rows of one argument, a value of each.
                let call = &body[..8];
                at = 8 + 2;
                str_at(&body, &mut at);
                let rows = u32_at(&body, &mut at);
                out.extend(call);
                out.extend(rows.to_le_bytes());
                for _ in 0..rows {
                    u32_at(&body, &mut at);
                    match body[at] {
                        4 => {
                            at += 1;
                            let s = str_at(&body, &mut at);
                            out.push(4);
                            put_str(&mut out, &s.to_uppercase());
                        }
                        _ => {
                            at += 1;
                            out.push(0);
                        }
                    }
                }
                write(&mut output, 4, &out).unwrap();
            }
            7 => return,
            _ => {}
        }
    }
}
```

The example in the source, `plugin/examples/plugin_example`, serves
the same through the SDK, with functions that fail as a program may.

## When a plug-in fails

A call fails, and the statement with it, as its program does; the
database goes on.

| SQLSTATE | When |
|---|---|
| the program's | It answered ERROR of a class a routine may raise: 22, 23, 38, 39, 42 or 2F |
| 38000 | It answered ERROR of another class |
| 38T01 | Its process ended, crashed or closed a pipe; the next call starts another |
| 38T02 | The call ran past PLUGINS.CALL_TIMEOUT_MS; its process is stopped |
| 38T03 | It broke protocol 1; its process is stopped |
| 38T04 | It sent more than PLUGINS.MAX_MESSAGE_BYTES in a message, or PLUGINS.MAX_CALL_BYTES in a call |
| 38T05 | It cannot be started: its directory is under none of PLUGINS.DIRECTORIES, or its program is missing |
| 38T06 | Its manifest breaks a rule of format 1 |
| 38T10 | A table or index of it could not be told a change made or undone; it is damaged until the database opens again |
| 38T11 | A host plug-in failed, so the node stopped until it starts again ([Host Plug-ins](04-plugins-host.md)) |
| 40001 | A process of it stopped while the transaction wrote a table of its engine; the transaction is rolled back, to run again |

A statement cancelled cancels its plug-in's call. The database sends
CANCEL and waits PLUGINS.CANCEL_GRACE_MS for an answer before it stops
the process. At most PLUGINS.PROCESSES processes of an extension run at
once. HELLO tells a program PLUGINS.MEMORY_BYTES, the memory it may use;
what it sends is reserved of the database's memory budget as it is read.
A program starts in its directory with no environment but PATH and the
temporary directory's variables.

```sql
VALUES (FAIL('no such thing'));
```

```
turborel: SQLCODE -301 (SQLSTATE 22012): FAIL failed: no such thing
```

## What is loaded

INFORMATION_SCHEMA.EXTENSIONS shows each extension, its version, the key
that signed its package, and its processes and calls since the database
opened; EXTENSION_OBJECTS each object it declares; and
EXTENSION_PACKAGES the database's lock of packages. EXPLAIN ends with a
line naming the plug-in calls a plan makes.

```sql
SELECT OBJECT_TYPE, OBJECT_NAME, IS_DETERMINISTIC FROM INFORMATION_SCHEMA.EXTENSION_OBJECTS
    WHERE OBJECT_NAME IN ('TWICE', 'TAXICAB') ORDER BY OBJECT_NAME;
```

```
 OBJECT_TYPE                                                  | OBJECT_NAME                                                  | IS_DETERMINISTIC
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 METRIC                                                       | TAXICAB                                                      | NO
 FUNCTION                                                     | TWICE                                                        | YES
(2 rows)
```

## Other plug-in points

A metric is named as a built-in one is, by VECTOR_DISTANCE and by a
vector index, whose search sends a call of many candidates at once. An
embedding model is the TYPE of `CREATE EMBEDDING MODEL`. An
authenticator's name is a mechanism clients of the server may ask for;
the user it accepts is the user the client named. A backup target's
scheme names its targets in BACKUP DATABASE, ARCHIVE.TARGET and the
other places a location is given, when no provider the program
registered has that scheme. Its provider (`Database::extension_target`)
may also be registered with another database, as one that restores a
backup before any extension is created in it. A partitioning strategy
is found by name by the program hosting the database
(`Database::extension_strategy`), as the partitioning that takes it is
built on the same interface. The later kinds, and the points no plug-in
may serve, are in [Kinds of Plug-in](04-plugins-kinds.md).
