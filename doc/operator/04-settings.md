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

DEFAULT gives a setting its default again. A session sets its own value
with [SET](../sql/statements/set-setting.md); only the owner sets the
database's ([SET DATABASE](../sql/statements/set-database.md)).

## When a change takes effect

| Takes effect | Of |
|---|---|
| at once | most: the next statement sees it |
| from the next transaction | LOCK_WAIT, TRANSACTION_POLICY |
| when the server starts | SERVER.HOST, SERVER.PORT, SERVER.SOCKET, SERVER.TLS and its files, SERVER.TRUST, SERVER.PASSWORD_ITERATIONS, STORAGE.POOL_BYTES |
| only when the database is made | STORAGE.PAGE_SIZE, STORAGE.CHECKSUM, STORAGE.ENCODINGS, LOG.SEGMENT_SIZE |

A setting fixed when the database is made is refused after:

```
turborel: SQLCODE -103 (SQLSTATE 2A000): STORAGE.PAGE_SIZE is fixed when the database is created
```

## Reading settings

INFORMATION_SCHEMA.SETTINGS shows each setting's value for the session
reading it, and where it comes from:

```sql
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME LIKE 'SERVER.%';
```

A change of the database's value is told to the diagnostics,
`setting_changed`.
