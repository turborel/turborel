# Connections

[Contents](README.md)

## TCP

The server takes TCP connections at the hosts or addresses of
SERVER.HOST, by commas, 127.0.0.1 by default, on SERVER.PORT, 7422; port
0 lets the system choose one, which the server says as it starts.
SERVER.HOST empty takes no TCP connection. Over TCP, a password is proved
only once TLS secures the connection ([Passwords](05-users.md#passwords)).

## Unix sockets and named pipes

SERVER.SOCKET names a Unix socket the server takes connections at too,
on Linux and macOS; on Windows, a named pipe. Only this machine reaches
either, so a connection over one needs no TLS, and a password may be
proved over it.

```
$ turboreld --dir db --set SERVER.SOCKET=/run/turborel/socket
$ turborel --socket /run/turborel/socket -U ann -W
```

On Windows, a pipe's name begins `\\.\pipe\`:

```
> turboreld --dir db --set SERVER.SOCKET=\\.\pipe\turborel
> turborel --socket \\.\pipe\turborel -U ann -W
```

The server makes the pipe as the first of its name, so it does not
start if another process holds the name (`cannot listen at
\\.\pipe\turborel`), and while it listens no other process can take
the name. It refuses clients of other machines. Who of this machine may
connect is as Windows secures a pipe by default: the server's own user,
the administrators and the system fully; any other user may open the
pipe to read only, so can send nothing, and is closed once
SERVER.LOGIN_TIMEOUT_MS runs out. Each such client holds a connection
until then, under SERVER.MAX_CONNECTIONS; where other users of the
machine are not trusted, keep SERVER.TRUST_LOCAL OFF, as it is by default,
so every client proves who it is.

A client waits for a pipe busy as long as its connect timeout; once the
pipe is open, its reads and writes are not bounded: `--read-timeout`
does not apply to a named pipe.

## TLS

SERVER.TLS ON offers TLS on the TCP port; REQUIRED takes TCP connections
over TLS alone. SERVER.TLS_CERTIFICATE names the PEM file of the
server's certificate chain, SERVER.TLS_KEY that of its private key. A
client trusts the certificates of the PEM file it is given, `--tls`,
and the platform's authorities only if asked, `--tls-system-roots`
([The Command-Line Client](../client/01-turborel.md)); it checks the
server's certificate against the host it names, or `--server-name`.

```
$ turboreld --dir db --set SERVER.TLS=REQUIRED \
      --set SERVER.TLS_CERTIFICATE=/etc/turborel/cert.pem --set SERVER.TLS_KEY=/etc/turborel/key.pem
$ turborel -h localhost -U ann -W --tls /etc/turborel/ca.pem
```

A client that does not use TLS is refused, and so is one that does not
trust the certificate:

```
turborel: SQLCODE -919 (SQLSTATE 08004): this server takes TCP connections over TLS only
turborel: SQLCODE -919 (SQLSTATE 08006): the TLS handshake failed: invalid peer certificate: UnknownIssuer
```

The server's certificate is one a certificate authority issued, as the
client trusts: a self-signed certificate used as its own authority is
refused. The client trusts the authorities of its file alone, not the
platform's.

SERVER.TLS_CLIENT_CA names the PEM file of the authorities whose
certificates clients may present, each then logging in as the user its
common name names ([Certificates](05-users.md#certificates)); empty, its
default, no client is asked for one.

### Certificates renewed

The server takes its certificate, key and client authorities again
without a restart, for the connections secured after; those secured go
on as they are:

- when SERVER.TLS_CERTIFICATE, SERVER.TLS_KEY or SERVER.TLS_CLIENT_CA is
  changed, as by the owner's SET DATABASE, as the next connection begins
  TLS;
- on SIGHUP, on Unix, from the same files, for a certificate renewed in
  place ([Signals](02-turboreld.md#signals)).

Files that cannot be taken, as a key that does not fit its certificate,
leave the server with those it had; it tells the diagnostics,
`tls_reload_failed`, once for each change, and `tls_reloaded` once it
takes new ones ([Diagnostics](08-diagnostics.md)). So a certificate and
its key changed by two statements, one after the other, may be refused
between them, and taken after the second. SERVER.TLS itself, and so
whether TLS is offered at all, takes effect as the server starts.

## Limits on connections

| Setting | Limit |
|---|---|
| SERVER.MAX_CONNECTIONS | connections at once, 256 |
| SERVER.WORKERS | statements run at once, one per processor |
| SERVER.MAX_STATEMENTS | prepared statements of a connection, 1,024 |
| SERVER.MAX_MESSAGE_BYTES | the longest message a client sends, 16 MiB |
| SERVER.BATCH_BYTES | about the bytes of rows in a batch sent, 64 KiB |
| SERVER.LOGIN_TIMEOUT_MS | the milliseconds a client has to begin TLS and authenticate, the two together, from when the connection is taken, 60,000 |
| SERVER.MAX_LOGINS | logins checked at once, apart from SERVER.WORKERS, 4 |
| SERVER.LOGIN_DELAY_MS | the longest a login waits after failed ones from its address, 10,000; 0, no wait |
| SERVER.IDLE_TIMEOUT_MS | the milliseconds the server waits on a client before closing it, its transaction rolled back; 0, no limit |
| SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS | the milliseconds the server waits for the next request of a client whose transaction is active, before closing it, its transaction rolled back; 0, no limit |

A connection past SERVER.MAX_CONNECTIONS is refused, told why once its
TLS, if any, is begun:

```
turborel: SQLCODE -919 (SQLSTATE 08004): the server has as many connections as it takes
```

At most SERVER.MAX_REFUSALS (16) such refusals are made at once; a
connection past them too is closed at once, told nothing, so clients that flood the server hold
no more of it than that. Either way the diagnostics are told, as
`connections_refused`, at most once a minute
([Diagnostics](08-diagnostics.md)).

## Logins

A client logging in holds little of the server. Until it is
authenticated, it may send no message longer than 16 KiB, or
SERVER.MAX_MESSAGE_BYTES if that is less, and a message's memory is
taken as its bytes arrive, not as its length says. Its password is
checked, a slow hash on purpose, on threads of the server's apart from
those statements run on, at most SERVER.MAX_LOGINS at once: clients
that log in, or try to, never keep the statements of those logged in
waiting.

After a login fails, the next from the same address waits before it is
checked: 100 ms, doubled with each failure after, up to
SERVER.LOGIN_DELAY_MS. Logins of one address waiting take turns, one a
delay, so connections in parallel guess no faster. A login accepted
forgets the address's failures, and so do ten minutes without one. An
IPv6 address counts by its /64; the clients of a Unix socket or named
pipe count as one address. A user is never locked out: that would let anyone lock out
anyone.

## Clients gone

A client whose machine or network fails never closes its connection,
and its session would hold its transaction's locks while the server
waited. The server turns TCP keepalive on for each connection it takes,
so the system finds such a client out, and the server rolls its
session back. The system's own times apply, on Linux two hours idle,
then nine probes 75 seconds apart, unless the server's settings say
otherwise, each read as a connection is taken, 0 for the system's own:

| Setting | What |
|---|---|
| SERVER.KEEPALIVE_IDLE_MS | how long a connection is idle before the first probe |
| SERVER.KEEPALIVE_INTERVAL_MS | how long between probes unanswered |
| SERVER.KEEPALIVE_PROBES | the probes unanswered after which the connection is closed |

A client gone is found out after about the idle time, then the interval
times the probes. The systems count these times in whole seconds, so a
time is rounded up to one; a time is at most 32767 seconds and the
probes at most 127, Linux's limits. Linux, macOS and Windows set all
three, as do the BSDs and illumos. Windows sets the idle time and the
interval together: given one, the other is Windows' own default (two
hours idle, a second between probes), whatever its registry says; its
probes are set from Windows 10 version 1703 only, before which it
always sends ten. On a system that cannot set a time given, every TCP
connection fails as it is taken, and the failure is told as the event
`accept_failed` ([Diagnostics](08-diagnostics.md)). SERVER.IDLE_TIMEOUT_MS
closes any client that keeps the server waiting longer;
SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS closes, sooner, one that sends
nothing while its transaction is active, its locks held:

```
turborel: SQLCODE -919 (SQLSTATE 08006): the connection went past SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS, 60000 ms, with its transaction active and nothing from the client: closed
```

## Ending a connection

The database's owner ends a connection by its ID in
INFORMATION_SCHEMA.CONNECTIONS, [END CONNECTION](../sql/statements/end-connection.md),
or cancels the statement it runs, [CANCEL
CONNECTION](../sql/statements/cancel-connection.md); a program that serves
a database does so through `Server::operator`, taken before it serves,
`Operator::end(ID)` and `Operator::cancel(ID)`. Ending one cancels the
statement it runs, as a client's cancel does, then closes it, its
session rolled back; the client is told:

```
turborel: SQLCODE -919 (SQLSTATE 08006): the server's operator ended the connection
```

A connection whose user is dropped ends at its next request
([Users](05-users.md#a-users-sessions)).

The database's owner sees the connections served, and the server's
counts, in INFORMATION_SCHEMA.CONNECTIONS and INFORMATION_SCHEMA.SERVER
([Information Schema](../sql/information-schema.md)). Of a server whose
one connection is the owner's own, over TCP without TLS, its statement
the one running:

```sql
SELECT ID, USER_NAME, SECURE, STATE FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY ID;
SELECT WORKERS_BUSY, STATEMENTS_WAITING, CONNECTIONS, REFUSED, CLOSED_UNTOLD FROM INFORMATION_SCHEMA.SERVER;
```

```
                   ID | USER_NAME                                                    | SECURE | STATE
----------------------+--------------------------------------------------------------+--------+---------
                    1 | ANN                                                          | NO     | RUNNING
(1 row)
         WORKERS_BUSY |   STATEMENTS_WAITING |          CONNECTIONS |              REFUSED |        CLOSED_UNTOLD
----------------------+----------------------+----------------------+----------------------+----------------------
                    1 |                    0 |                    1 |                    0 |                    0
(1 row)
```
