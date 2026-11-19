# Settings, Pipelines and Pools

[Contents](README.md)

The client library connects a program as [The Client
Library](02-library.md) describes, one server and one request at a time.
This chapter describes what it adds for programs that run in more than
one place or under load: connection settings read from outside the
program, several servers to choose among, TLS by mode, pipelines of
requests, pools of connections, and the SCRAM key cache. Each is in the
crate `turborel-client`, on the standard library alone; TLS needs its
feature `tls`, and SCRAM its feature `scram`.

## Connecting by settings

`settings::Settings` says where the servers are, as whom to connect and
how. `Settings::connect` connects as it says, and gives a connection
like `target::connect_to`'s.

```rust
use turborel_client::settings::Settings;

let settings = Settings::from_uri("turborel://ann@db1.example:7422,db2.example/?tls_mode=verify-full&tls_roots=/etc/turborel/ca.pem")?;
let mut conn = settings.connect()?;
println!("{}, as {}", conn.server(), conn.authorization());
// turborel 0.0.0 (alpha), as ANN
```

A password the settings give is proved by SCRAM-SHA-256 with the feature
`scram`, else sent whole by the mechanism `password`; with no password,
the user is proved by the client's certificate if one is named, else
trusted as a local client. A secret mechanism is refused over plain TCP
before anything is sent, as by `connect` (SQLSTATE 28000).

### Where settings come from

Each setting has a name, the same everywhere it is written:

| Setting | What it says | Default |
|---|---|---|
| `host` | The servers, by name or address, comma-separated; each may carry its port as `name:port`, an IPv6 address as `[address]:port`, and a path names a Unix socket (on Windows, a named pipe) | `localhost` |
| `port` | The port of each host that carries none: one for all, or one each, comma-separated | 7422 |
| `user` | The user to connect as | none: it must be given |
| `password` | The user's password | the password file's |
| `tls_mode` | `disable`, `prefer`, `require`, `verify-ca` or `verify-full` | `prefer` |
| `channel_binding` | `disable`, `prefer` or `require` | `prefer` |
| `application_name` | The program's name | none |
| `connect_timeout` | Whole seconds each step of connecting waits; 0 for no limit | 30 |
| `service` | The section of the service file to read | none |
| `service_file` | The service file | `~/.turborel_service.conf` |
| `passfile` | The password file | `~/.turborel_pass` |
| `target` | `any`, `read-write` or `read-only` | `any` |
| `load_balance` | `disable`, the hosts tried in order, or `random` | `disable` |
| `tls_roots` | A PEM file of the certificate authorities trusted | none |
| `tls_system_roots` | `true` to trust the platform's authorities too | `false` |
| `tls_certificate`, `tls_key` | PEM files of the client's certificate chain and key, to present | none |
| `server_name` | The name the server's certificate is checked against | the host's |

Settings come in layers, and each setting is taken from the first layer
that has it:

1. the program's own, an `Options` it fills or a connection URI;
2. the section of the service file the program or the environment names;
3. the environment's variables;
4. the defaults.

```rust
use turborel_client::settings::{Options, Settings, TLS_MODE};

let mut options = Options::new();
options.set(TLS_MODE, "verify-full")?;
let settings = Settings::resolve(&options)?;
```

The environment's variables are TURBOREL_HOST, TURBOREL_PORT,
TURBOREL_USER, TURBOREL_PASSWORD, TURBOREL_TLS_MODE,
TURBOREL_APPLICATION_NAME, TURBOREL_CONNECT_TIMEOUT, TURBOREL_SERVICE,
TURBOREL_SERVICE_FILE and TURBOREL_PASSFILE, each the setting of its
name. A variable set but empty gives nothing. `Settings::resolve_in`
reads the environment through a function the program gives, as a test
does to give one of its own.

A setting that cannot be read, a service file that cannot be read or
has no section of the name, and settings that name no user are refused
with SQLSTATE 08001. `application_name` is sent as the connection
attribute `application`, which the database's owner sees in
INFORMATION_SCHEMA.CONNECTIONS and CONNECTION_ATTRIBUTES.

### The connection URI

```text
turborel://[<user>[:<password>]@][<host>[:<port>][,<host>[:<port>]]...][/][?<setting>=<value>[&<setting>=<value>]...]
```

Its parts are percent-encoded, so `p%40ss` is the password `p@ss` and
`%2Frun%2Fturborel.sock` a Unix socket; an IPv6 address is written in
brackets. A server serves one database, so the URI names none, and has
no path. A URI of another scheme, of a path, of a setting not in the
table above or of a pair without `=` is refused (SQLSTATE 08001).

```text
turborel://ann@db1.example,db2.example:7500/?target=read-write&connect_timeout=5
turborel://ann@%2Frun%2Fturborel.sock
turborel://ann@[::1]:7422/?tls_mode=disable
```

### The service file

A service file names sets of settings, so a program, or a person at the
command line, names a service rather than repeating them. It holds
sections, each `[name]` on a line of its own, then lines
`setting = value`. A line beginning with `#` is a comment, and blank
lines are skipped. A section cannot set `service` or `service_file`. A
malformed line is refused, its file and line named.

```text
# ~/.turborel_service.conf
[production]
host = db1.example,db2.example
user = ann
tls_mode = verify-full
tls_roots = /etc/turborel/ca.pem
target = read-write

[reports]
host = replica.example
user = reporter
target = read-only
```

```rust
use turborel_client::settings::{Options, SERVICE, Settings};

let mut options = Options::new();
options.set(SERVICE, "production")?;
let conn = Settings::resolve(&options)?.connect()?;
```

### The password file

A password the settings do not give is looked up in the password file,
by the server and the user each connection is made to. Each line is
`host:port:user:password`; a field `*` matches anything; `\` takes the
character after it as it is, so `\:` is a colon in a field and `\*` a
star; a line beginning with `#` is a comment. The first line that
matches is taken. A host that is a Unix socket is matched by its path,
and the default port.

```text
# ~/.turborel_pass
db1.example:7422:ann:first-password
*:*:reporter:report\:ing
```

A password file should be readable by its owner alone. The client
cannot check that, as the standard library has no portable way to read
a file's owner or permissions, and the client has no code of one
platform. It reads the file only if it is a regular file (or a link to
one), and its owner answers for keeping it private.

## Several servers

`host` may name several servers. They are tried in the order given, or,
with `load_balance=random`, in an order drawn at random for each
connection, so a program's connections spread over them. A name is
tried at each address it resolves to, each step within the connect
timeout. A server that cannot be reached, refuses the user, or is not of
the role asked is passed for the next. With none kept, the failure
names each address and why (SQLSTATE 08001; 28000 if the last refused
the user).

`target` asks for a role. `read-write` keeps a server that takes changes:
a server alone, or its replicated group's primary. `read-only` keeps a
replica that follows its group, which refuses every change. The server
tells each session whether it takes changes, as its session status
SERVER_READ_ONLY, so any user's connection finds its role with no
query. Of a server that does not tell it, the role is read from
INFORMATION_SCHEMA.REPLICAS, which shows it to the database's owner
alone, so another user's session cannot see a replica's role, and a
connection of its asking for either is passed. `role::role` gives the
role a connection's session sees.

```rust
use turborel_client::settings::Settings;

let primary = Settings::from_uri("turborel://ann@db1.example,db2.example,db3.example/?target=read-write")?.connect()?;
```

## TLS by mode

`tls_mode` says how a connection over TCP uses TLS. A Unix socket or a
named pipe, which only its machine reaches, uses none.

| Mode | TLS | The server's certificate |
|---|---|---|
| `disable` | Never | |
| `prefer` | If the server takes it, else plain TCP | Not checked |
| `require` | Always | Not checked |
| `verify-ca` | Always | Signed by an authority trusted |
| `verify-full` | Always | Signed by an authority trusted, and of the server's name |

`prefer` and `require` keep a connection from being overheard, not from
a server in the middle, and `verify-full` keeps both. `verify-ca` and
`verify-full` need the authorities trusted: `tls_roots`, or
`tls_system_roots` with the feature `system-roots`. The name checked is
`server_name`, else the host's. Under `prefer`, a server reached whose
TLS handshake fails is connected to again over plain TCP; one that
cannot be reached is not tried twice. Without the feature `tls`, only
`disable` and `prefer` connect, `prefer` always over plain TCP.

`channel_binding` says whether SCRAM binds its exchange to the TLS
connection carrying it, so a server in the middle cannot relay it
([The Client Library](02-library.md)). `prefer` binds where the
connection has a binding; `disable` binds nothing; `require` refuses,
before any connection, settings whose mechanism binds nothing, a
connection not over TLS, or a password not proved by SCRAM (SQLSTATE
28000). `Scram::channel_binding` sets the same of a mechanism the
program makes.

## Pipelines

A pipeline sends a connection's requests without waiting for each
reply, so a batch costs one round trip, not one a request. `pipeline`
borrows a connection; `query` and `execute` queue requests; `run` writes
them and gives one outcome each, in order. The server answers each
request alone and in turn, so a failure is its request's own, and the
requests after it run.

```rust
use turborel_client::pipeline::Piped;

let insert = conn.prepare("INSERT INTO STAFF (ID, NAME) VALUES (?, ?)")?;
let mut pipeline = conn.pipeline();
pipeline.query("INSERT INTO STAFF (ID, NAME) VALUES (1, 'ANN')");
pipeline.execute(&insert, &[Value::Exact(Decimal::new(2, 0)), Value::Char("BOB".into())]);
pipeline.query("SELECT ID, NAME FROM STAFF ORDER BY ID");
for outcome in pipeline.run()? {
    match outcome {
        Piped::Done(done) => println!("{done:?}"),
        Piped::Failed(failure) => println!("{}", failure.message),
        Piped::Skipped => println!("skipped"),
    }
}
```

Requests are written a window of 64 KiB at a time
(`pipeline::PIPELINE_WINDOW`), its replies read before the next window
is written, so a long batch never leaves client and server each waiting
for the other to read. Under `set_autocommit` each request is its own
transaction, as `query` makes it.

`on_error(OnError::Skip)` makes a batch stop at its first failure, the
requests before it kept. The client sets a savepoint before each request
but the first; the requests after a failure are rolled back to the
savepoint set after it, and those of a window not yet written are never
sent. Each is `Piped::Skipped`. Such a batch is one transaction. Under
autocommit, of a batch that began none, it is committed as the batch
ends, and otherwise it stays as a statement would leave it. What a rollback
undoes is undone; a session's settings, which no rollback undoes, are
not.

`run` fails, giving no outcome, if the connection is lost (SQLSTATE
08006), or if a skipping batch cannot be undone.

## Pools

`pool::Pool` keeps a bounded set of connections that a program's threads
share. Its clones are the same pool. `get` hands out an idle connection
that answers, or makes one while fewer than `max` are made, or waits for
one to come back. The connection it gives, a `Pooled`, is used as a
connection and goes back to the pool as it is dropped.

```rust
use turborel_client::pool::{Pool, PoolConfig};
use turborel_client::settings::Settings;

let pool = Pool::new(Settings::from_uri("turborel://ann@db.example")?, PoolConfig { max: 20, ..PoolConfig::default() });
let worker = pool.clone();
std::thread::spawn(move || {
    let mut conn = worker.get()?;
    conn.query("UPDATE STAFF SET SALARY = SALARY * 1.1 WHERE ID = 1")?;
    conn.query("COMMIT")
});
```

| `PoolConfig` | What it says | Default |
|---|---|---|
| `max` | The most connections made at once | 10 |
| `max_idle` | The most kept idle | 10 |
| `idle_timeout` | How long one may stay idle | 10 minutes |
| `max_lifetime` | How long one lives from when it was made | 1 hour |
| `wait` | How long `get` waits for one to come back | 30 seconds |
| `check` | Whether an idle connection is checked to answer before it is handed out | yes |
| `reset` | Statements run as each comes back, after the pool's own | none |

A connection comes back reset to the session it logged in with, by the
protocol's Reset, one request that ends its transaction, prepared
statements, cursors, temporary tables and settings and keeps its user;
autocommit is off, then `reset`'s statements run. Its check is a Ping,
which the server answers without a worker. Of a server that takes
neither, the check is ROLLBACK and the reset one pipeline: its
transaction rolled back, the statements it prepared closed and
destroyed, its user, role and schema those of its login and its time
zone local, and a setting a borrower changed by SET stays as it set it,
unless `reset` sets it back. A connection lost, whose reset fails, that has lived its time, or
beyond `max_idle` is closed, never handed out again. One idle past
`idle_timeout`, or that does not answer its check, is closed as `get`
meets it, and another taken or made.

`get` fails with SQLSTATE 08001 if no connection comes back within
`wait`, or with the failure of making one. A pool is made of `Settings`
or of any closure that makes a connection, through the trait
`pool::Source`. It starts no thread, and each wait is the waiting
thread's.

## The SCRAM key cache

A SCRAM login derives its keys from the password by PBKDF2, the slow
step, made slow so a stolen exchange is slow to guess from. A server
sends a user the same salt and iterations at each login, so
`scram::KeyCache` keeps the keys a password derived after a login the
server proved, and a later login that meets the same salt and iterations
skips the derivation. `Settings` keeps one for every connection it
makes, so a pool's connections and a program's reconnections share it.
A mechanism the program makes takes one by `Scram::with_cache`.

```rust
use turborel_client::scram::{KeyCache, Scram};

let cache = KeyCache::new();
let mut scram = Scram::new("ann's").with_cache(&cache);
let conn = tls::connect("db.example:7422", ("db.example", &config), "ANN", &mut scram)?;
let (hits, misses) = cache.lookups();
// hits: 0, misses: 1; a second connection by `scram`: hits 1
```

An entry is found by the salt, the iterations and a tag of the password
made under a random secret of the cache's own. So a wrong password
finds nothing, and the cache holds neither the password nor anything it
could be guessed from offline. It holds 64 entries at most
(`KEY_CACHE_ENTRIES`), the oldest dropped first, and drops an entry when
a login that used it fails. It lives in the process's memory alone.
