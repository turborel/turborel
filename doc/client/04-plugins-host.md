# Host Plug-ins

[Contents](README.md)

A host plug-in serves a point beneath the database. The pages it
writes, the log it commits to, its clock, its files, its keys and the
connections a server takes can all come from a program of another
party. It speaks protocol 1, as an extension does, and is written with
the same SDK ([Plug-ins](04-plugins.md)). The node declares it, never
SQL, and starts it before the database is opened from its files. Each
object it serves is used as the host's own.

A call crosses a pipe, so no host object is asked inside a commit's
critical section, a latch or a lock. What the database needs there is a
value it keeps, which a call filled before. Each section below states a
kind's cost in calls, and choosing to pay it is the operator's
decision.

## Declaring a host plug-in

A host plug-in's manifest declares host objects and nothing else.

```
HOST <kind> <name> [ ( <option> = <number or 'text'>, ... ) ]
```

```
NAME VAULT; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE 'vault';
HOST ENCODING XOR (ID = 9001, OVERHEAD = 1);
HOST HASH FUNCTION FNV (ID = 9001, LENGTH = 8);
HOST LOG STORAGE FILELOG (BATCH = 256);
HOST CLOCK SYSTEM (EVERY = 10);
```

The setting PLUGINS.HOST names each host plug-in and its directory,
separated by `;`. A digest after `@` pins its program's SHA-256, which
a plug-in bringing package cryptography must have. The host sets it, in
`turboreld`'s configuration file or with `--set`, and SQL may not.

```
turboreld --dir /srv/db --set "PLUGINS.HOST=VAULT=/opt/vault"
```

```
listening at 127.0.0.1:5433
```

A program embedding the database sets it and starts its host plug-ins
before it opens the files.

```rust
let db = Database::new();
db.set_setting("PLUGINS.HOST", &Value::Char("VAULT=/opt/vault".into()))?;
db.start_host_plugins()?;
db.open_files(&db.file_system()?, Path::new("/srv/db"), &OpenOptions::default())?;
```

INFORMATION_SCHEMA.HOST_PLUGINS shows the database's owner each object
started, with its license as the manifest declares it, and `turborel
admin host-plugins` lists them from the command line.

```
turborel admin host-plugins
```

```
PLUGIN_NAME  VERSION  OBJECT_TYPE  OBJECT_NAME  IS_PINNED  LICENSE  PROCESSES_RUNNING  CALLS  STOPPED
-----------  -------  -----------  -----------  ---------  -------  -----------------  -----  -------
VAULT        1.0.0    CLOCK        SYSTEM       NO         MIT                      1     52  NULL
(1 row)
```

## When a host plug-in fails

A host object either answers or stops the node. A process that ends,
breaks the protocol, sends too much or outlasts PLUGINS.CALL_TIMEOUT_MS
stops it, and so does an error the kind does not take as an answer.
Every statement after fails, and no page or record is written.

```sql
SELECT COUNT(*) FROM T;
```

```
ERROR 38T11: the host plug-in VAULT failed, so the node stopped until it starts again: the plug-in process ended: its output ended
```

`turboreld` stops serving and exits with the reason. At its next start
the database is recovered from its log, as after a crash, and every
commit made durable before is there.

## Encodings and checksums

```
HOST ENCODING <name> (ID = <n>, OVERHEAD = <bytes>, CONFIDENTIAL = 0 | 1)
HOST HASH FUNCTION <name> (ID = <n>, LENGTH = <bytes>)
```

An encoding transforms each page and record the database writes.
STORAGE.ENCODINGS names it, and the files it wrote name its ID, so a
database is read through the program that wrote it. A page written is
one ENCODE call and a page read one DECODE call, made with no latch
held. A page that does not decode fails its read and the node goes on.
A checksum is named by STORAGE.CHECKSUM and gives digests of its LENGTH,
1 to 64 bytes, one call a digest. The SDK's traits are `PageEncoding`
and `Checksum`.

```sql
SET DATABASE STORAGE.ENCODINGS = 'XOR';
```

```
OK
```

## The clock

```
HOST CLOCK <name> (EVERY = <milliseconds>)
```

The database asks the program its time every EVERY milliseconds, 10 if
not written, one call each, and keeps the latest. CURRENT_TIMESTAMP and
every commit read that copy, which never goes back. The SDK's trait is
`TimeSource`.

## The log and the catalog

```
HOST LOG STORAGE <name> (BATCH = <records>, DURABLE = 0 | 1)
HOST CATALOG STORAGE <name>
```

Log storage keeps the database's log. A commit appends in memory, and
its sync sends every record not yet sent in one APPEND call, so the
commits waiting together are made durable in one call. A commit whose
sync the program fails is in doubt (08007) until the next start
recovers. Recovery reads BATCH records a call, 256 if not written.
Catalog storage records each image of the catalog in one call. The
SDK's traits are `LogStore` and `CatalogKeeper`.

## Spill

```
HOST SPILL <name>
```

A sort, a hash join or a grouping past MEMORY.WORK_BYTES spills to the
program. Each spill area keeps one process while it lives. A run is
pushed a batch at a time and read back 1024 records a call, and a
table's read of a key is one call. The SDK's trait is `SpillStore`.

## The file system

```
HOST FILE SYSTEM <name>
```

Every file the database opens, creates, removes or locks is the
program's, named by its path. A page read or written is one call.
`Database::open`, restore and `turboreld` open through it. The SDK's
trait is `Files`.

## Keys and package cryptography

```
HOST KEYRING <name>
HOST PACKAGE CRYPTO <name>
```

A keyring gives a 256-bit key by name, one call each, which encryption
keeps while it reads with it. `turboreld` takes keys from the first
keyring started, else from `--keys`. Package cryptography digests and
verifies packages. Its program's digest must be pinned in PLUGINS.HOST,
and the host's own cryptography checks it before each process starts.
The SDK's traits are `Keys` and `Crypto`.

```
turboreld --dir /srv/db --set "PLUGINS.HOST=SIGNER=/opt/signer@3f8a...c41e"
```

## Jobs

```
HOST BACKGROUND JOB <name>
```

A job runs as the database's own, labelled `turborel-<name>` and shown
in INFORMATION_SCHEMA.JOBS. Each run is one call, whose answer says when
to run again. The SDK's trait is `Job`.

```sql
SELECT LABEL FROM INFORMATION_SCHEMA.JOBS WHERE LABEL = 'turborel-tick' AND RUNS > 0;
```

```
LABEL
-------------
turborel-tick
(1 row)
```

## Connections

```
HOST LISTENER <name>
HOST SERVED CONNECTIONS <name>
```

A listener takes connections beside SERVER.HOST and SERVER.SOCKET. Each
connection keeps one process while it is open, so PLUGINS.PROCESSES
bounds them. ACCEPT waits up to 50 milliseconds for a client and is
asked again until one comes. The bytes pass in RELAY calls, one an
exchange while the client talks and one each 50 milliseconds while it
is quiet. No socket passes between processes, so the program may speak
any transport to its clients. Served connections end and cancel
connections by id, one call each. The SDK's traits are `Accept` and
`Connections`.

## What is no kind

The optimizer's rules are an extension's kind, over a plan form of
their own ([Optimizer Rules and Statistics](04-plugins-planner.md)). A
wire protocol is a listener's to translate. Settings give the policies and
the cost model. [RULES.md D255](../../RULES.md) argues each.
