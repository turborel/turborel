# SET

[Statements](README.md)

## NAME

SET: change a session setting *(extension)*

## SYNOPSIS

```
<set setting statement> ::=
    SET <setting name> { = | TO } { <value specification> | DEFAULT }

<setting name> ::= <identifier> [ { . <identifier> }... ]
```

The `<value specification>` is a literal, USER, CURRENT_USER,
SESSION_USER or SYSTEM_USER, or, in a statement prepared, a `?`.

## DESCRIPTION

SET gives the session its own value of a setting, over the database's,
until the session ends or sets it again. DEFAULT takes the database's
value again. A setting's name is taken in any case.

Only some settings may be set by a session; INFORMATION_SCHEMA.SETTINGS
shows which (IS_SESSION_SETTABLE), what each takes (VALUE_KIND), when a
change takes effect (TAKES_EFFECT), and where the value a session sees
comes from (VALUE_SOURCE: SESSION, DATABASE or DEFAULT).
[Appendix C](../../appendix/c-settings.md) lists every setting.

SET begins no transaction, may run within one, and is not undone by
ROLLBACK.

`SET DATABASE`, followed by a setting's name, is
[SET DATABASE](set-database.md), the database's value.

## RULES

1. The setting exists.
2. A session may set it.
3. The value is of the setting's kind and range.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | There is no such setting, or it is the database's alone |
| 22000 | -301 | The value is not of the setting's kind or range |

## EXAMPLES

```sql
SET STATEMENT.TIMEOUT_MS = 5000;
SELECT SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'STATEMENT.TIMEOUT_MS';
SET STATEMENT.TIMEOUT_MS TO DEFAULT;
SELECT SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'STATEMENT.TIMEOUT_MS';
```

```
OK
 SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------
 5000                                                         | SESSION
(1 row)
OK
 SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------
 0                                                            | DEFAULT
(1 row)
```

Values and settings refused:

```sql
SET LOCKS.WAIT = 'SOMETIMES';
SET MEMORY.BUDGET_BYTES = 1048576;
SET NO_SUCH = 1;
```

```
turborel: SQLCODE -301 (SQLSTATE 22000): LOCKS.WAIT takes ONE OF ON, OFF, not 'SOMETIMES'
turborel: SQLCODE -103 (SQLSTATE 2A000): MEMORY.BUDGET_BYTES is the database's, which a session does not set for itself
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no setting NO_SUCH
```

## STANDARD

An extension: the standards define no settings. SET TIME ZONE
[ISO/IEC 9075:1992 §16.5] sets the session's value of the setting
SESSION.TIME_ZONE.

## SEE ALSO

[SET DATABASE](set-database.md), [SET TIME ZONE](set-time-zone.md),
[Appendix C: Settings](../../appendix/c-settings.md),
[Settings](../../operator/04-settings.md)
