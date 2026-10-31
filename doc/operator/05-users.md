# Users and Authentication

[Contents](README.md)

## Users and the owner

A user is a name a client of the server may connect as, proving it by a
password, or by a certificate ([Certificates](#certificates)). The
database keeps, of each, not the password but a verifier made of it:
PBKDF2 of SERVER.PASSWORD_ITERATIONS iterations, 600,000 by default,
with a salt of its own, and of that two keys, SCRAM-SHA-256's StoredKey
and ServerKey. Nothing in the database tells the password, nor lets
anyone log in who has read the verifier.

One user is the database's **owner**: the first made, unless another is
made it. The owner alone manages the users in SQL, by [CREATE
USER](../sql/statements/create-user.md), [ALTER
USER](../sql/statements/alter-user.md) and [DROP
USER](../sql/statements/drop-user.md), and sets the database's settings.
The owner cannot be dropped until another owns the database.

The owner hands the database on to another user, on a running server,
by [ALTER DATABASE](../sql/statements/alter-database.md):

```
ALTER DATABASE OWNER TO bob;
```

No session is ended; the owner's go on as the owner no more. With no
server running, `turboreld user owner NAME` does the same
([Managing users from turboreld](#managing-users-from-turboreld)).

What a user may do is the privileges granted to its name, as of any
authorization identifier ([GRANT](../sql/statements/grant.md)).

## A user's sessions

A session lasts while its user does. Once a user is dropped, each
session of it ends at its next statement, which is refused, its
transaction rolled back and its connection closed:

```
turborel: SQLCODE -919 (SQLSTATE 28000): the user BOB was dropped: its connections end
```

A new password ends no session: it proved who it is as it logged in,
and the next login takes the new password. To end a session at once,
its operator ends its connection ([Connections](06-network.md#ending-a-connection)).

## Passwords

A client proves a password by one of these mechanisms, which the server
all takes:

| Mechanism | How |
|---|---|
| `scram-sha-256-plus` | SCRAM-SHA-256-PLUS (RFC 5802, RFC 7677), over TLS: as `scram-sha-256`, the exchange bound to the TLS connection. Over TLS 1.3 by `tls-exporter` (RFC 9266), keying material of that connection alone; over TLS 1.2 by `tls-server-end-point` (RFC 5929), the hash of the certificate the server presented. The command-line client uses it over TLS. |
| `scram-sha-256` | SCRAM-SHA-256 (RFC 5802, RFC 7677): the client proves it knows the password, and the server that it knows the verifier, by a challenge and response; the password never travels. The command-line client uses it over a Unix socket or named pipe. |
| `password` | the password is sent, inside TLS or over a Unix socket or named pipe, and checked against the verifier |

Each is taken only over TLS, a Unix socket or a named pipe, never in
the clear over TCP: the client refuses to begin so, and the server to
take it. A SCRAM exchange does not give the password away, but one
overheard lets the eavesdropper try passwords against it at leisure.
SERVER.SCRAM_WITHOUT_TLS ON lets the server take `scram-sha-256` over
plain TCP all the same, for a network its operator trusts; it is OFF by
default, and a client sends it so only if its program says.

Over TLS, the binding keeps a server in the middle from relaying the
exchange: it holds one TLS connection to the client and another to the
server, so what the client binds is not what the server binds, and the
login is refused as a wrong password is. The client names the binding in
its first message, and the server takes the one it names where the
connection has it:

| Binding | Where | What is bound |
|---|---|---|
| `tls-exporter` | TLS 1.3; the client's choice there | 32 bytes exported of the connection's own keys |
| `tls-server-end-point` | TLS 1.2 and 1.3; the client's choice over TLS 1.2 | the hash of the certificate the server presented |

Over TLS 1.2 there is no `tls-exporter`. Each connection is bound to the
certificate its handshake presented, a certificate taken again included
([Certificates renewed](06-network.md#certificates-renewed)). A
certificate whose signature uses no hash, as Ed25519's, or two, defines
no `tls-server-end-point`: over TLS 1.2 the client then asks for
`scram-sha-256`, saying it could bind, and the server takes it. A
client saying so where the server does offer a binding is refused, as a
server in the middle hiding the binding would make it:

```
turborel: SQLCODE -919 (SQLSTATE 28000): SCRAM: the client believes the server offers no channel binding, which it does: a downgrade
```

A password is taken as its UTF-8 bytes, without the SASLprep of RFC
5802: the same of an ASCII password.

A client gives a wrong user or password the same answer, after as long,
by either mechanism: `the user or the password is wrong`, SQLSTATE
28000. After a failed login, the next from the same address waits,
longer with each failure, up to SERVER.LOGIN_DELAY_MS, ten seconds by
default ([Logins](06-network.md#logins)); no user is ever locked out.

## Certificates

With SERVER.TLS_CLIENT_CA naming the PEM file of the authorities it
trusts, the server asks each TLS client for a certificate. A client
presenting one of those authorities' logs in, by the mechanism
`certificate`, as the user its subject's common name names, read as an
identifier, as a user's name is: `CN=ann` logs in as ANN, if ANN is a
user. One name, compared whole, is the mapping: whoever the authorities
certify as ANN may log in as ANN, and no one else by that certificate.
A certificate of other authorities fails the TLS handshake; a client
presenting none is served as before, by a password. A certificate as
another user, or of a name that is no user's, is refused:

```
turborel: SQLCODE -919 (SQLSTATE 28000): the certificate is not of the user named
```

```
$ turboreld --dir db --set SERVER.TLS=ON --set SERVER.TLS_CERTIFICATE=/etc/turborel/cert.pem \
      --set SERVER.TLS_KEY=/etc/turborel/key.pem --set SERVER.TLS_CLIENT_CA=/etc/turborel/clients.pem
$ turborel -h localhost -U ann --tls /etc/turborel/ca.pem --certificate ann.pem --key ann.key
```

## Trusted local clients

With SERVER.TRUST_LOCAL ON, the server takes a client of its own machine as
the user it names, with no password, a user or not. It is OFF by
default: every client proves who it is. LOCAL suits a machine of one
operator; on a machine of many accounts, any of them could connect as
the owner.

## Managing users from turboreld

`turboreld --dir PATH user COMMAND` manages the users of a database no
server holds; it sets a database up before it is served. A password is
typed without its echo at a terminal, or else read from standard input.

| Command | Does |
|---|---|
| `user add NAME [--owner]` | adds a user, the owner if asked |
| `user password NAME` | gives a user a new password |
| `user drop NAME` | drops a user |
| `user owner NAME` | makes a user the owner |
| `user list` | lists the users, the owner marked |

```
$ turboreld --dir /var/lib/turborel/main user add ann --owner
user ANN added, the owner
$ turboreld --dir /var/lib/turborel/main user add bob
user BOB added
$ turboreld --dir /var/lib/turborel/main user list
ANN (owner)
BOB
```

A name is an identifier, so `ann` is ANN. Each change of a user, or of
the owner, is told to the diagnostics, `user_changed` and
`owner_changed`; no verifier ever is.

## The owner of a database kept nowhere

A database kept nowhere, served by `turboreld --memory`, has no
directory to set up first. `--owner NAME` makes NAME its first user, and
so its owner, as the server starts; its password is typed without its
echo at a terminal, or else read from standard input, as `user add`
reads one. Without it, such a database has no user, so no owner: no one
could manage its users or set its settings in SQL.

```
$ turboreld --memory --owner ann --set SERVER.SOCKET=/run/turborel/socket
password:
turboreld: listening at 127.0.0.1:7422, /run/turborel/socket
```

`--owner` is for `--memory` alone: a database kept in a directory is
given its owner by `user add NAME --owner`.
