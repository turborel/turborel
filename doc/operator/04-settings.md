# Settings

[Contents](README.md)

## Scopes

A setting has the database's value, kept with the database, and may
have a session's own: a session's value of a setting a session may set
stays above the database's for that session alone. The server's settings,
SERVER.*, are the database's too, kept with it. [Settings](../appendix/c-settings.md)
lists every setting, its default, what values it takes, and when a
change takes effect.

## SET DATABASE

The database's value is set as the server starts, by `--set NAME=VALUE`
or a [configuration file](02-turboreld.md#configuration-files), or on a
running server by its owner, in SQL:

```sql
SET DATABASE LOCKS.WAIT = 'ON';
SET DATABASE SERVER.MAX_CONNECTIONS TO 512;
SET DATABASE LOCKS.WAIT = DEFAULT;
```

```
OK
OK
OK
```

DEFAULT gives a setting its default again. A session sets its own value
with [SET](../sql/statements/set-setting.md); only the owner sets the
database's ([SET DATABASE](../sql/statements/set-database.md)).

## When a change takes effect

INFORMATION_SCHEMA.SETTINGS tells when a change of each setting takes
effect, in its column TAKES_EFFECT, by one of five values; Appendix C
says the same in words:

| TAKES_EFFECT | Appendix C | When | Of |
|---|---|---|---|
| STATEMENT | at once | for each statement that begins after the change | most |
| TRANSACTION | from the next transaction | for each transaction that begins after the change | LOCKS.WAIT, TRANSACTION.STRICT |
| CONNECTION | from the next connection | for each connection the server takes after the change | SERVER.MAX_MESSAGE_BYTES, SERVER.TLS_CERTIFICATE, SERVER.TLS_KEY, SERVER.TLS_CLIENT_CA, SERVER.SCRAM_WITHOUT_TLS, SERVER.KEEPALIVE_* |
| START | at the next start | as the server next starts, or the database is next opened | SERVER.HOST, SERVER.PORT, SERVER.SOCKET, SERVER.TLS, SERVER.TRUST_LOCAL, SERVER.PASSWORD_ITERATIONS, STORAGE.POOL_BYTES |
| CREATION | only when the database is made | as the database is made in its directory; fixed after | STORAGE.PAGE_BYTES, STORAGE.CHECKSUM, STORAGE.ENCODINGS, LOG.SEGMENT_BYTES |

```sql
SELECT SETTING_NAME, TAKES_EFFECT FROM INFORMATION_SCHEMA.SETTINGS
    WHERE TAKES_EFFECT <> 'STATEMENT' ORDER BY TAKES_EFFECT, SETTING_NAME;
```

```
 SETTING_NAME                                                 | TAKES_EFFECT
--------------------------------------------------------------+--------------
 SERVER.KEEPALIVE_IDLE_MS                                     | CONNECTION
 SERVER.KEEPALIVE_INTERVAL_MS                                 | CONNECTION
 SERVER.KEEPALIVE_PROBES                                      | CONNECTION
 SERVER.MAX_MESSAGE_BYTES                                     | CONNECTION
 SERVER.SCRAM_WITHOUT_TLS                                     | CONNECTION
 SERVER.TLS_CERTIFICATE                                       | CONNECTION
 SERVER.TLS_CLIENT_CA                                         | CONNECTION
 SERVER.TLS_KEY                                               | CONNECTION
 LOG.SEGMENT_BYTES                                            | CREATION
 STORAGE.CHECKSUM                                             | CREATION
 STORAGE.ENCODINGS                                            | CREATION
 STORAGE.PAGE_BYTES                                           | CREATION
 SERVER.HOST                                                  | START
 SERVER.PASSWORD_ITERATIONS                                   | START
 SERVER.PORT                                                  | START
 SERVER.SOCKET                                                | START
 SERVER.TLS                                                   | START
 SERVER.TRUST_LOCAL                                           | START
 STORAGE.POOL_BYTES                                           | START
 LOCKS.WAIT                                                   | TRANSACTION
 TRANSACTION.STRICT                                           | TRANSACTION
(21 rows)
```

A setting fixed when the database is made is refused after:

```
turborel: SQLCODE -103 (SQLSTATE 2A000): STORAGE.PAGE_BYTES is fixed when the database is created
```

A database kept in memory, as `turboreld --memory` serves, is created
in no directory, so it takes none of them: `--set` of one is refused as
the server starts, and SET DATABASE of one once the database holds a
schema.

A change of a setting taken at the next start waits for it: STARTED_VALUE
shows the value the server, or the database as it opened, started with,
and IS_PENDING is YES while the value set differs from it. Of a setting
not taken so, or not started yet, STARTED_VALUE is null and IS_PENDING
NO.

```sql
COMMIT;
SET DATABASE SERVER.TLS = 'REQUIRED';
SELECT SETTING_NAME, SETTING_VALUE, STARTED_VALUE, IS_PENDING FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME = 'SERVER.TLS';
COMMIT;
SET DATABASE SERVER.TLS = DEFAULT;
```

```
OK
OK
 SETTING_NAME                                                 | SETTING_VALUE                                                | STARTED_VALUE                                                | IS_PENDING
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+------------
 SERVER.TLS                                                   | REQUIRED                                                     | OFF                                                          | YES
(1 row)
OK
OK
```

## Reading settings

INFORMATION_SCHEMA.SETTINGS shows each setting's value for the session
reading it, and where it comes from:

```sql
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME LIKE 'SERVER.MAX%' ORDER BY SETTING_NAME;
```

```
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------------------------------------------------------+--------------
 SERVER.MAX_CONNECTIONS                                       | 512                                                          | DATABASE
 SERVER.MAX_LOGINS                                            | 4                                                            | DEFAULT
 SERVER.MAX_MESSAGE_BYTES                                     | 16777216                                                     | DEFAULT
 SERVER.MAX_REFUSALS                                          | 16                                                           | DEFAULT
 SERVER.MAX_STATEMENTS                                        | 1024                                                         | DEFAULT
(5 rows)
```

A change of the database's value is told to the diagnostics,
`setting_changed`.

## A session's own

A session may turn some things on or off for itself, over the
database's value, with [SET](../sql/statements/set-setting.md):
whether its commits wait for the log, LOG.COMMIT_SYNC; whether it takes
the function call syntax, SQL.FUNCTION_CALLS; and each of the
optimizer's rules, OPTIMIZER and the rule's name, as to see a plan made
without it. A rule a program adds to the optimizer has its setting too.

```sql
SET LOG.COMMIT_SYNC = 'OFF';
SET OPTIMIZER.PREDICATE_TRANSITIVITY = 'OFF';
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME IN ('LOG.COMMIT_SYNC', 'OPTIMIZER.PREDICATE_TRANSITIVITY', 'SQL.FUNCTION_CALLS')
    ORDER BY SETTING_NAME;
SET LOG.COMMIT_SYNC TO DEFAULT;
SET OPTIMIZER.PREDICATE_TRANSITIVITY TO DEFAULT;
```

```
OK
OK
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------------------------------------------------------+--------------
 LOG.COMMIT_SYNC                                              | OFF                                                          | SESSION
 OPTIMIZER.PREDICATE_TRANSITIVITY                             | OFF                                                          | SESSION
 SQL.FUNCTION_CALLS                                           | OFF                                                          | DEFAULT
(3 rows)
OK
OK
```

A commit of LOG.COMMIT_SYNC OFF returns once its commit record is
written, before the log is synced: its changes are seen by others at
once, and a crash before the log is next synced, by a commit that
waits, a definition, a checkpoint or the database closing, loses it, as
it loses every commit after it. The database stays whole either way.
