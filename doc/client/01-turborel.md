# The turborel Command

[Contents](README.md)

## Synopsis

```
turborel [options]
```

`turborel` connects to a turborel server and runs SQL there: typed,
given on its command line, or read from files. It shows a query's rows
as a table as they come, and each statement's outcome.

## Options

| Option | Meaning |
|---|---|
| `-h`, `--host HOST` | the server's host; 127.0.0.1 by default |
| `-p`, `--port PORT` | its port; 7422 by default |
| `--socket PATH` | the server's Unix socket instead (Unix only) |
| `--tls FILE` | connect over TLS, trusting the certificates of the PEM file FILE |
| `--server-name NAME` | the name the server's certificate is checked against; the host by default |
| `-U`, `--user NAME` | who connects; the user of the environment by default |
| `-W`, `--password` | prove it by a password, typed without its echo |
| `-c`, `--command SQL` | run SQL's statements; more than once for more |
| `-f`, `--file FILE` | run FILE's statements; more than once for more |
| `--stop-on-error` | end the run at the first statement that fails |
| `--null TEXT` | show a null value as TEXT; nothing by default |
| `--help` | print the usage |
| `--version` | print the release |

A password may be given in the environment variable TURBOREL_PASSWORD
instead of typed. Without either, the client asks the server to trust
it as the user it names, which a server does only of a client of its own
machine, and only if its setting SERVER.TRUST is LOCAL; it is OFF by
default. A password is sent only over TLS or a Unix socket.

## Typed sessions

Without `-c` or `-f`, the client reads statements from its standard
input. Typed at a terminal, it begins with a banner, of its release and
the server's, and prompts for each statement: `turborel>`, and
`turborel-` for a line that goes on with one begun. A statement runs
once a semicolon ends it; the end of input, Ctrl-D (on Windows Ctrl-Z
then Enter), quits.

```
turborel 0.0.0 (alpha), the command-line client
Copyright 2026, Turborel contributors.
Distributed under the BSD 2-Clause License.

Connected to turborel 0.0.0 (alpha) at 127.0.0.1:7491, as ANN.
End each statement with a semicolon; end of input (Ctrl-D) quits.

turborel> SELECT ID, NAME
turborel-   FROM STAFF
turborel-  WHERE ID < 3;
           ID | NAME
--------------+--------------------------------
            1 | Ada
            2 | Grace
(2 rows)
turborel>
```

A statement's outcome is shown after it: its rows, or `OK`, or a warning
or error with its SQLCODE, SQLSTATE and message, and where in the
statement it is:

```
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.NOPE is not defined [X3.135-1986 §5.4 SR5] (at character 14)
OK, with a warning: SQLSTATE 01007
```

A statement left without its semicolon at the end of the input runs as
the input ends. Semicolons in strings, delimited identifiers and
comments do not end a statement.

## Running files and commands

`-c` and `-f` run their statements, each in the order given, in one
session, and then the client ends: `turborel -c "SELECT ..." -f
setup.sql -c "COMMIT"`. Input piped to the client runs the same way,
without banner or prompts. A statement that fails is told, and the run
goes on, unless `--stop-on-error` ends it there.

```
$ turborel -U ann -f run.sql
-----
   1
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22012): a divisor shall not be 0 [X3.135-1986 §5.9 GR5]

-----
   2
(1 row)
$ echo $?
1
```

A transaction a run leaves open is ended as the session ends: committed,
unless the setting SESSION_END is ROLLBACK.

## Connections

The client connects as it starts, its connection the default one.
[CONNECT](../sql/statements/connect.md), [SET
CONNECTION](../sql/statements/set-connection.md) and
[DISCONNECT](../sql/statements/disconnect.md) make others, choose among
them and end them: the client runs these statements itself, as the
SQL-agent. A connection CONNECT makes is to an address, and proves its
user as the first did, a password typed asked for again.

## Interrupting a statement

Ctrl-C cancels the statement running: the server stops it, and it
fails with SQLCODE -920, SQLSTATE TB920, undone as a statement that
fails is. In a
typed session the client then prompts again; a run of `-c`, `-f` or
piped input ends there. Before the client has connected, Ctrl-C ends it.

```
turborel: SQLCODE -920 (SQLSTATE TB920): the statement was cancelled
```

## Exit status

0 if every statement succeeded; 1 if one failed, or the client could not
connect, or its command line is wrong; 130 if Ctrl-C ended it before it
connected.

```
$ turborel -p 1 -U ann -c "VALUES 1"
turborel: SQLCODE -919 (SQLSTATE 08006): cannot connect: Connection refused (os error 61)
$ echo $?
1
```
