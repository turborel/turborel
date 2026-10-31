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
| `--socket PATH` | the server's Unix socket instead; on Windows its named pipe, as `\\.\pipe\turborel` |
| `--tls FILE` | connect over TLS, trusting the certificates of the PEM file FILE |
| `--tls-system-roots` | connect over TLS, trusting the platform's certificate authorities too, or alone without `--tls` |
| `--server-name NAME` | the name the server's certificate is checked against; the host by default |
| `--certificate FILE` | over TLS, present the certificate chain of the PEM file FILE |
| `--key FILE` | the private key of that certificate, in the PEM file FILE |
| `-U`, `--user NAME` | who connects; the user of the environment by default |
| `-W`, `--password` | prove it by a password, typed without its echo |
| `-c`, `--command SQL` | run SQL's statements; more than once for more |
| `-f`, `--file FILE` | run FILE's statements; more than once for more |
| `--stop-on-error` | end the run at the first statement that fails |
| `--null TEXT` | show a null value as TEXT, and import a field written TEXT as null; nothing by default |
| `--import FILE` | insert FILE's rows into the table `--table` names; more than once for more, in turn with `-c` and `-f` ([Importing](#importing)) |
| `--table NAME` | the table rows are imported into |
| `--format FORMAT` | how an imported file is written: `csv`, the default, or `tsv` |
| `--header` | an imported file's first line names the columns of its fields |
| `--delimiter CHAR` | the character between an imported file's fields; a comma for `csv`, a tab for `tsv` by default |
| `--batch ROWS` | commit each ROWS rows imported; 1000 by default, 0 for all at once |
| `--skip-bad-rows` | tell a row that fails to import, and import the rest |
| `--connect-timeout SECONDS` | wait at most SECONDS to connect, begin TLS and for each step of the login; 30 by default, 0 for no limit |
| `--read-timeout SECONDS` | wait at most SECONDS for each reply of the server; 0, no limit, by default, as a statement may run long |
| `--help` | print the usage |
| `--version` | print the release |

A password may be given in the environment variable TURBOREL_PASSWORD
instead of typed. The client proves it by SCRAM-SHA-256, never sending
it, and takes the server only once the server has proved it knows the
user's verifier ([Passwords](../operator/05-users.md#passwords)). It
does so only over TLS, a Unix socket or a named pipe: over plain TCP the
client refuses to begin, and connects to nothing (SQLSTATE 28000). Over
a named pipe, `--read-timeout` does not apply: once the pipe is open,
the client waits on it as long as it must.

Over TLS the client trusts the authorities of the file `--tls` names,
and only them, unless `--tls-system-roots` asks for the platform's too:
the keychain's on macOS, the system store's on Windows, the system's
bundle elsewhere; or instead those of the files the environment's
SSL_CERT_FILE and SSL_CERT_DIR name, if either is set. They are never
trusted unless asked for: a platform trusts many authorities, any of
which could certify a server of any name, where a server of one's own is
usually certified by an authority of one's own. A platform's store that
cannot be read, or holds no certificate, is a failure that says so:

```
turborel: SQLCODE -919 (SQLSTATE 08006): cannot trust the platform's certificate authorities: it holds none
```

Without a password, a certificate presented, `--certificate` and
`--key`, both, with `--tls` or `--tls-system-roots`, proves the user
its common name names, to a server that asks for one
([Certificates](../operator/05-users.md#certificates)).
With a password, the password proves the user, the certificate presented
still. Without either, the client asks the server to trust it as the
user it names, which a server does only of a client of its own machine,
and only if its setting SERVER.TRUST_LOCAL is ON; it is OFF by default.

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

Connected to turborel 0.0.0 (alpha) at 127.0.0.1:7422, as ANN.
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

A query that completes with a warning, as a set function that leaves
out a null does, says so after its count of rows: `(1 row, with a
warning: SQLSTATE 01003)`.

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
unless the setting SESSION.COMMIT_AT_END is OFF. A commit that fails then, as
one of a deferred constraint not satisfied (SQLSTATE 40002), is told,
the transaction rolled back, and the program exits with 1.

## Importing

`--import FILE` inserts the rows of FILE into the table `--table`
names, in its turn among the statements `-c` and `-f` name. A file is
CSV, as RFC 4180 writes it, unless `--format tsv` says it is
tab-separated:

- **CSV**: a record a line, ended by CR LF or LF, the last one's
  optional; fields split at commas, or at the character `--delimiter`
  names, their spaces kept. A field may be enclosed in double quotes,
  and must be to hold the delimiter, a quote or a line break; a quote in
  it is written twice. A quote in a field not enclosed, or anything but
  the delimiter after a closing quote, is refused.
- **Tab-separated**: a record a line, fields split at tabs, never
  quoted; a field holds no tab and no line break.

A field not enclosed in quotes is null if it is written as `--null`
says: by default an empty field, so `1,,x` has a null between, and
`1,"",x` the empty string. Text is UTF-8; a byte order mark beginning
the file is passed over. A record is at most 16 MiB.

With `--header`, the first line names the columns the fields go into,
each name read as an identifier: `name` is the column NAME, `"Name"`
written in the file as `"""Name"""` the column Name; a column not named
takes its default. Without it, each record has a field for each column
of the table, in order. Each value is cast to its column's type, as a
dynamic parameter's is; a string too long for a character string
column is refused (SQLSTATE 22001), not cut.

The rows are inserted through one prepared INSERT, `--batch` rows to a
transaction, each batch committed. An import begins with no transaction
active: one a statement before it left open is refused, as the import
would commit it. A row that fails stops the import: the client tells
the record, the line it began on and why, and how many rows were
committed before; the batch under way is rolled back. The run then goes
on, unless `--stop-on-error` ends it. With `--skip-bad-rows`, a row
that fails, or has too many or too few fields, is told and skipped, and
the rest are imported; a run with a row skipped exits with 1. A record
that cannot be read, as one whose quote is never closed, stops the
import even so.

A table, and a file with a header, imported into it:

```sql
CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(20), HIRED DATE);
COMMIT
```

```
OK
OK
```

```
$ cat staff.csv
id,name,hired
1,Ada,1980-01-02
2,"Hopper, Grace",
$ turborel -U ann --import staff.csv --table STAFF --header
OK, 2 rows imported
```

A row that fails stops the import, its batch rolled back; told to, the
client skips it, and imports the rest:

```
$ cat more.csv
3,Eve,1999-12-31
4,Bob,someday
5,Carol,2001-05-06
$ turborel -U ann --import more.csv --table STAFF
turborel: more.csv: record 2, line 2: SQLCODE -301 (SQLSTATE 22018): 'someday' is no DATE literal's string [ISO/IEC 9075:1992 §6.10 GR9a ii, GR10a ii, GR11a ii]; 0 rows committed, 1 rolled back
$ turborel -U ann --import more.csv --table STAFF --skip-bad-rows
turborel: more.csv: record 2, line 2: SQLCODE -301 (SQLSTATE 22018): 'someday' is no DATE literal's string [ISO/IEC 9075:1992 §6.10 GR9a ii, GR10a ii, GR11a ii]; skipped
OK, 2 rows imported, 1 skipped
```

```sql
SELECT ID, NAME, HIRED FROM STAFF ORDER BY ID
```

```
           ID | NAME                 | HIRED
--------------+----------------------+------------
            1 | Ada                  | 1980-01-02
            2 | Hopper, Grace        | NULL
            3 | Eve                  | 1999-12-31
            5 | Carol                | 2001-05-06
(4 rows)
```

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
A cancel the server does not answer within 10 seconds is given up, and
Ctrl-C may be pressed again.

A server that does not answer within `--read-timeout` loses the
connection, and the statement fails:

```
turborel: SQLCODE -919 (SQLSTATE 08006): cannot receive: the server did not answer in time (Resource temporarily unavailable (os error 35))
```

```
turborel: SQLCODE -920 (SQLSTATE TB920): the statement was cancelled
```

## Exit status

0 if every statement and import succeeded; 1 if one failed, or an
import skipped a row, or the client could not connect, or its command
line is wrong; 130 if Ctrl-C ended it before it connected.

```
$ turborel -p 1 -U ann -c "VALUES 1"
turborel: SQLCODE -919 (SQLSTATE 08006): cannot connect: Connection refused (os error 61)
$ echo $?
1
```
