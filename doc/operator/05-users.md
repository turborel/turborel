# Users and Authentication

[Contents](README.md)

## Users and the owner

A user is a name a client of the server may connect as, proving it by a
password. The database keeps, of each, not the password but a verifier
made of it, PBKDF2 of SERVER.PASSWORD_ITERATIONS iterations, 600,000 by
default; nothing in the database tells the password.

One user is the database's **owner**: the first made, unless another is
made it. The owner alone manages the users in SQL, by [CREATE
USER](../sql/statements/create-user.md), [ALTER
USER](../sql/statements/alter-user.md) and [DROP
USER](../sql/statements/drop-user.md), and sets the database's settings.
The owner cannot be dropped until another owns the database.

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

A password is sent only over TLS or a Unix socket, never in the clear
over TCP: the client refuses to send it so, and the server to take it. A client gives a wrong user or password the same answer, after
as long: `the user or the password is wrong`, SQLSTATE 28000. After a
failed login, the next from the same address waits, longer with each
failure, up to SERVER.LOGIN_DELAY_MS, ten seconds by default
([Logins](06-network.md#logins)); no user is ever locked out.

## Trusted local clients

With SERVER.TRUST LOCAL, the server takes a client of its own machine as
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
