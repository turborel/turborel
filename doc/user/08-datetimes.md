# Dates, Times and Intervals

[Contents](README.md)

## The datetime types

DATE is a day; TIME a time of day; TIMESTAMP a day and a time; each of
TIME and TIMESTAMP may be WITH TIME ZONE, and keep a displacement from
UTC. CURRENT_DATE, CURRENT_TIME and CURRENT_TIMESTAMP give the instant a
statement began ([Data Types](../sql/02-data-types.md)).

```sql
CREATE TABLE MEETINGS (TITLE VARCHAR(20), STARTS TIMESTAMP WITH TIME ZONE, LASTS INTERVAL HOUR TO MINUTE);
INSERT INTO MEETINGS VALUES ('Review', TIMESTAMP '2026-11-02 09:30:00+01:00', INTERVAL '1:30' HOUR TO MINUTE);
```

## Time zones

A session has a time zone, +00:00 unless [SET TIME
ZONE](../sql/statements/set-time-zone.md) sets another. A datetime
written without one is taken in the session's; one WITH TIME ZONE keeps
its own. AT TIME ZONE shows an instant in another:

```sql
SELECT TITLE, STARTS AT TIME ZONE INTERVAL '-05:00' HOUR TO MINUTE FROM MEETINGS;
```

```
 TITLE                |
----------------------+----------------------------------
 Review               | 2026-11-02 03:30:00.000000-05:00
(1 row)
```

A displacement is from -12:59 to +14:00, the easternmost time zone in
use, as SQL:2023 requires; SQL-92 stopped at +13:00. One beyond is
refused, SQLSTATE 22009:

```sql
SELECT TITLE, STARTS AT TIME ZONE INTERVAL '14:00' HOUR TO MINUTE FROM MEETINGS;
SELECT TITLE, STARTS AT TIME ZONE INTERVAL '14:30' HOUR TO MINUTE FROM MEETINGS;
```

```
 TITLE                |
----------------------+----------------------------------
 Review               | 2026-11-02 22:30:00.000000+14:00
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22009): a time zone displacement is not from -12:59 to +14:00 [ISO/IEC 9075-2:2023 §6.42 GR5b ii 2 A]
```

## Intervals

An interval is a span of time, of years and months, or of days, hours,
minutes and seconds: the two do not mix, a month having no fixed number
of days. `INTERVAL '1:30' HOUR TO MINUTE` is an hour and a half.

## Arithmetic

A datetime plus an interval is a datetime; the difference of two, in
parentheses, given the fields wanted, an interval:

```sql
SELECT TITLE, STARTS, STARTS + LASTS AS ENDS FROM MEETINGS;
SELECT EXTRACT(HOUR FROM STARTS), (DATE '2026-12-25' - DATE '2026-11-02') DAY FROM MEETINGS;
```

```
 TITLE                | STARTS                           | ENDS
----------------------+----------------------------------+----------------------------------
 Review               | 2026-11-02 09:30:00.000000+01:00 | 2026-11-02 11:00:00.000000+01:00
(1 row)
         |
---------+-----
       9 |  53
(1 row)
```

A month added keeps the day, so January 31 plus a month, which would be
February 31, is refused, SQLSTATE 22008.
