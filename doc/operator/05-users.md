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

## Passwords

A password is sent only over TLS or a Unix socket, never in the clear
over TCP. A client gives a wrong user or password the same answer, after
as long: `the user or the password is wrong`, SQLSTATE 28000.

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
