# SET COLLATION

[Statements](README.md)

## NAME

SET COLLATION: set the session's collation

## SYNOPSIS

```
<set session collation statement> ::=
    SET COLLATION <value specification> [ FOR SQL_TEXT ]
  | SET NO COLLATION [ FOR SQL_TEXT ]
```

## DESCRIPTION

SET COLLATION makes a collation override the columns' in the session's own
statements, until SET NO COLLATION. An explicit COLLATE still wins. Views,
constraints and defaults keep what they were defined with, and a prepared
statement keeps the collation it was prepared under.

It begins no transaction. While it is set, an index of another collation
serves no comparison, so such queries may scan.

## RULES

1. The value is a character string, its spaces trimmed, naming a collation.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2H000 | -301 | The value names no collation, or the collation was dropped since |
| 2A000 | -103 | The value is not a character string |
| 0A000 | -900 | FOR names another character set |

## EXAMPLES

```sql
CREATE TABLE S (NAME VARCHAR(20));
INSERT INTO S VALUES ('Bob');
SELECT COUNT(*) FROM S WHERE NAME = 'BOB';
SET COLLATION 'CASE_INSENSITIVE';
SELECT COUNT(*) FROM S WHERE NAME = 'BOB';
SELECT COUNT(*) FROM S WHERE NAME COLLATE UCS_BASIC = 'BOB';
SET NO COLLATION;
SELECT COUNT(*) FROM S WHERE NAME = 'BOB';
SET COLLATION 'NOPE';
```

```
OK
OK

----------------------
                    0
(1 row)
OK

----------------------
                    1
(1 row)

----------------------
                    0
(1 row)
OK

----------------------
                    0
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2H000): no collation NOPE [ISO/IEC 9075-2:2023 §19.10 GR1a]
```

## STANDARD

ISO/IEC 9075-2:2023 §9.15 SR4c, §19.10, Feature F693.

## SEE ALSO

[CREATE COLLATION](create-collation.md), [SET SCHEMA](set-schema.md)
