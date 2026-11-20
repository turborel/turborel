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
HOST HASH FUNCTION XXH64 (ID = 9001, LENGTH = 8);
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

## Settings of a host plug-in

A host plug-in's manifest may declare settings as an extension's does
([Plug-ins](04-plugins.md)), named after the plug-in. They are NODE
settings, which no other scope is allowed for, declared as the plug-in
starts. Like PLUGINS.HOST they are the host's to give, in the
configuration file, with `--set` or by `Database::set_setting`, and SQL
may not set them. A configuration file may name them before they are
declared, and a value that fits none is refused as the plug-in starts.

```
NAME VAULT; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE 'vault';
HOST BUFFER EVICTION KEEP (EVERY = 1000);
SETTING KEEP_PAGES TYPE INTEGER MIN 0 DEFAULT 0;
```

```
turboreld --dir /srv/db --set "PLUGINS.HOST=VAULT=/opt/vault" --set VAULT.KEEP_PAGES=64
```

The program reads them with each call, as an extension's program
reads its own. INFORMATION_SCHEMA.HOST_PLUGINS lists each, of type
SETTING.

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

A sort, a hash join or a grouping past its grant of MEMORY.WORK_BYTES spills to the
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

## Buffer eviction

```
HOST BUFFER EVICTION <name> [ ( EVERY = <milliseconds>, SAMPLE = <frames> ) ]
```

The pool chooses a page to evict on every miss, so it never calls the
program to choose. The program instead sets the policy the pool's own
evictor follows, which the pool reads with no lock. The evictor keeps,
for each frame, its page, the intervals of its last two uses and its
count of uses, where an interval passes each time a page is placed. A
policy weighs these into one score, and the evictor takes the highest
of a sample of frames.

| Field | Meaning |
|---|---|
| HISTORY | 1 counts a page's age from its last use, least recently used; 2 from the use before, LRU-2 |
| RECENCY | the weight of age |
| FREQUENCY | the weight taken off for each use, which keeps pages used often |
| ONCE | added for a page used once, so a scan's pages go first |
| SAMPLE | the frames each choice looks at, 32 if 0 |
| ranges | up to 64 ranges of a file's pages, each a weight, 100 being neutral |

A range of weight 0 keeps its pages, which are taken only when no other
frame looked at may be evicted. The SDK's trait is `EvictionAdvisor`,
added by `Plugin::host_eviction`, and `policies` builds the common ones
(`lru`, `lru_2`, `scan_resistant`, `frequent`, and `weighing` for a
range).

The policy costs one call when the plug-in is started, before any pool
is made, and one call each EVERY milliseconds after (1000 if it is not
written), none on any read. That call carries the pool's report: the
pages evicted since the last, at most 256, and SAMPLE frames (256 if it
is not written), a window that moves on each time, with each frame's
page, uses and last interval. The answer is a new policy, or none to
keep the last. A process that dies stops the node, and until it stops
the pool chooses by the last policy it was given.

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
wire protocol is a listener's to translate. Settings give the policies
and the cost model, and the buffer pool's eviction is programmed by a
BUFFER EVICTION object, not called. [RULES.md D255](../../RULES.md)
argues each.
