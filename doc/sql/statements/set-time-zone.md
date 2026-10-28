# SET TIME ZONE

[Statements](README.md)

## NAME

SET TIME ZONE: set the session's time zone

## SYNOPSIS

```
<set local time zone statement> ::= SET TIME ZONE <set time zone value>

<set time zone value> ::= <interval value expression> | LOCAL
```

## DESCRIPTION

SET TIME ZONE sets the session's time zone displacement: the offset from
UTC of the time zone the session works in. It is the displacement of a
datetime literal written without one, of CURRENT_TIME and
CURRENT_TIMESTAMP, and of a time or timestamp without time zone CAST to
one with time zone.

LOCAL sets it back to the database's: the setting TIME_ZONE, `+00:00`
unless the database sets another. A session begins with the database's.

The displacement is the session's own value of the setting TIME_ZONE;
INFORMATION_SCHEMA.SETTINGS shows it. SET TIME ZONE begins no
transaction, may run within one, and is not undone by ROLLBACK.

## RULES

1. The value is an INTERVAL HOUR TO MINUTE.
2. It is not null, and is from -12:59 to +13:00.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The value is not an INTERVAL HOUR TO MINUTE |
| 22009 | -301 | The value is null, or not from -12:59 to +13:00 |

## EXAMPLES

The same local time, written in two time zones:

```sql
CREATE TABLE MEETINGS (NAME CHARACTER VARYING(20), STARTS TIMESTAMP(0) WITH TIME ZONE);
SET TIME ZONE INTERVAL '+02:00' HOUR TO MINUTE;
INSERT INTO MEETINGS VALUES ('Berlin', TIMESTAMP '2026-03-01 09:00:00');
SET TIME ZONE INTERVAL -'05:00' HOUR TO MINUTE;
INSERT INTO MEETINGS VALUES ('New York', TIMESTAMP '2026-03-01 09:00:00');
SET TIME ZONE LOCAL;
SELECT NAME, STARTS, EXTRACT(TIMEZONE_HOUR FROM STARTS) FROM MEETINGS ORDER BY STARTS;
```

```
OK
OK
OK
OK
OK
OK
 NAME                 | STARTS                    |
----------------------+---------------------------+---------
 Berlin               | 2026-03-01 09:00:00+02:00 |       2
 New York             | 2026-03-01 09:00:00-05:00 |      -5
(2 rows)
```

Values refused:

```sql
SET TIME ZONE INTERVAL '14:00' HOUR TO MINUTE;
SET TIME ZONE INTERVAL '2' HOUR;
```

```
turborel: SQLCODE -301 (SQLSTATE 22009): INTERVAL '14:00' HOUR(2) TO MINUTE is no time zone displacement from -12:59 to +13:00 [ISO/IEC 9075:1992 §16.5 GR1b ii]
turborel: SQLCODE -103 (SQLSTATE 2A000): SET TIME ZONE takes an INTERVAL HOUR TO MINUTE, not INTERVAL HOUR(2) [ISO/IEC 9075:1992 §16.5 SR1]
```

## STANDARD

ISO/IEC 9075:1992 §16.5, the `<set local time zone statement>`. The
displacement a session begins with, which LOCAL sets again, is
implementation-defined [GR1a]: here, the database's setting TIME_ZONE.
Keeping it as a session's own setting, shown in
INFORMATION_SCHEMA.SETTINGS, is an extension. SQL-86 has no datetimes.

## SEE ALSO

[SET](set-setting.md), [SET DATABASE](set-database.md),
[Dates, Times and Intervals](../../user/08-datetimes.md),
[Appendix C: Settings](../../appendix/c-settings.md)
