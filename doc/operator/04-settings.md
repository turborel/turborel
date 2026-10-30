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

The database's value is set as the server starts, by `--set NAME=VALUE`,
or on a running server by its owner, in SQL:

```sql
SET DATABASE LOCK_WAIT = 'WAIT';
SET DATABASE SERVER.MAX_CONNECTIONS TO 512;
SET DATABASE LOCK_WAIT = DEFAULT;
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
effect, in its column TAKES_EFFECT, by one of four values; Appendix C
says the same in words:

| TAKES_EFFECT | Appendix C | When | Of |
|---|---|---|---|
| STATEMENT | at once | for each statement that begins after the change | most |
| TRANSACTION | from the next transaction | for each transaction that begins after the change | LOCK_WAIT, TRANSACTION_POLICY |
| START | when the server starts | as the server next starts, or the database is next opened | SERVER.HOST, SERVER.PORT, SERVER.SOCKET, SERVER.TLS and its files, SERVER.TRUST, SERVER.PASSWORD_ITERATIONS, STORAGE.POOL_BYTES |
| CREATION | only when the database is made | as the database is made in its directory; fixed after | STORAGE.PAGE_SIZE, STORAGE.CHECKSUM, STORAGE.ENCODINGS, LOG.SEGMENT_SIZE |

```sql
SELECT SETTING_NAME, TAKES_EFFECT FROM INFORMATION_SCHEMA.SETTINGS
    WHERE TAKES_EFFECT <> 'STATEMENT' ORDER BY TAKES_EFFECT, SETTING_NAME;
```

```
 SETTING_NAME                                                 | TAKES_EFFECT
--------------------------------------------------------------+--------------
 LOG.SEGMENT_SIZE                                             | CREATION
 STORAGE.CHECKSUM                                             | CREATION
 STORAGE.ENCODINGS                                            | CREATION
 STORAGE.PAGE_SIZE                                            | CREATION
 SERVER.HOST                                                  | START
 SERVER.PASSWORD_ITERATIONS                                   | START
 SERVER.PORT                                                  | START
 SERVER.SOCKET                                                | START
 SERVER.TLS                                                   | START
 SERVER.TLS_CERTIFICATE                                       | START
 SERVER.TLS_KEY                                               | START
 SERVER.TRUST                                                 | START
 STORAGE.POOL_BYTES                                           | START
 LOCK_WAIT                                                    | TRANSACTION
 TRANSACTION_POLICY                                           | TRANSACTION
(15 rows)
```

A setting fixed when the database is made is refused after:

```
turborel: SQLCODE -103 (SQLSTATE 2A000): STORAGE.PAGE_SIZE is fixed when the database is created
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
 SERVER.MAX_STATEMENTS                                        | 1024                                                         | DEFAULT
(4 rows)
```

A change of the database's value is told to the diagnostics,
`setting_changed`.
