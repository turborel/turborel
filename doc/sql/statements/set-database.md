# SET DATABASE

[Statements](README.md)

## NAME

SET DATABASE: change a database setting *(extension)*

## SYNOPSIS

```
<set database statement> ::=
    SET DATABASE <setting name> { = | TO } { <value specification> | DEFAULT }

<setting name> ::= <identifier> [ { . <identifier> }... ]
```

The `<value specification>` is a literal, USER, CURRENT_USER,
SESSION_USER or SYSTEM_USER, or, in a statement prepared, a `?`.

## DESCRIPTION

SET DATABASE sets the database's value of a setting, for every session,
and keeps it with the database's definitions, so it holds when the
database is opened again. DEFAULT gives the setting its default value
again. A setting's name is taken in any case. A session that set its own
value keeps it, over the database's.

When a change takes effect depends on the setting
(INFORMATION_SCHEMA.SETTINGS, TAKES_EFFECT): for each statement or
transaction that begins after it; when the server starts next or the
database is next opened; or, for a few, only as the database is created,
after which they are fixed. [Appendix C](../../appendix/c-settings.md)
lists every setting.

## RULES

1. Its user is the database's owner.
2. No transaction is active.
3. The setting exists, and is not one fixed once the database is
   created.
4. The value is of the setting's kind and range; a name it gives, of an
   engine, an index method, a hash function or encodings, is registered.

## PRIVILEGES

The database's owner alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -104 | Its user is not the database's owner |
| 25000 | -917 | A transaction is active |
| 2A000 | -103 | There is no such setting, or it is fixed once the database is created |
| 22000 | -301 | The value is not of the setting's kind or range, or names what is not registered |

## EXAMPLES

Run by the database's owner, ANN:

```sql
SET DATABASE STATEMENT_TIMEOUT = 60000;
SELECT SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'STATEMENT_TIMEOUT';
COMMIT;
SET DATABASE STATEMENT_TIMEOUT TO DEFAULT;
```

```
OK
 SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------
 60000                                                        | DATABASE
(1 row)
OK
OK
```

The SELECT began a transaction, so COMMIT ends it before SET DATABASE
runs again.

Run by BOB, who does not own the database:

```sql
SET DATABASE STATEMENT_TIMEOUT = 1;
```

```
turborel: SQLCODE -104 (SQLSTATE 2A000): only the database's owner runs SET DATABASE, and BOB is not it
```

## STANDARD

An extension: the standards define no settings.

## SEE ALSO

[SET](set-setting.md), [CREATE USER](create-user.md),
[Appendix C: Settings](../../appendix/c-settings.md),
[Settings](../../operator/04-settings.md)
