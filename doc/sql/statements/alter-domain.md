# ALTER DOMAIN

[Statements](README.md)

## NAME

ALTER DOMAIN: change a domain

## SYNOPSIS

```
<alter domain statement> ::=
    ALTER DOMAIN <domain name> <alter domain action>

<alter domain action> ::=
      SET DEFAULT <default option>
    | DROP DEFAULT
    | ADD <domain constraint>
    | DROP CONSTRAINT <constraint name>
```

The `<default option>` is that of [CREATE TABLE](create-table.md); the
`<domain constraint>`, that of [CREATE DOMAIN](create-domain.md).

## DESCRIPTION

ALTER DOMAIN changes a domain, and with it each column of the domain.

**SET DEFAULT** gives the domain a default, or a new one: each column of
the domain that has no default of its own takes it. No row changes.

**DROP DEFAULT** drops the domain's default. Each column of the domain
that had none of its own keeps it, as its own.

**ADD** adds a constraint to the domain, and so to each column of it. It
is checked of the rows there now: a row of a column of the domain that
breaks it refuses it.

**DROP CONSTRAINT** drops a constraint of the domain, and so of each of
its columns.

ALTER DOMAIN runs within the session's transaction: ROLLBACK undoes it.
It locks each table with a column of the domain, as
[ALTER TABLE](alter-table.md) does.

## RULES

1. The domain exists, and its user owns the domain's schema. A domain
   named without a schema is of the session's default schema.
2. SET DEFAULT: the default is of the domain's data type.
3. DROP DEFAULT: the domain has a default.
4. ADD: the constraint follows the rules of [CREATE DOMAIN](create-domain.md):
   its name is new in the schema, and it CASTs to no domain that is in
   usage by it, the domain's own included.
5. DROP CONSTRAINT: the constraint is the domain's.
6. The transaction is not READ ONLY.

## PRIVILEGES

The domain's schema is its user's. A constraint added needs the
REFERENCES a CHECK needs.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the domain's schema, or lacks a privilege the constraint needs |
| 23000 | -201 | A row breaks the constraint added |
| 25000 | -917 | The transaction is READ ONLY |
| TB911 | -911 | Another transaction holds a lock on a table of the domain |

## EXAMPLES

Of the domain MONEY and the table LEDGER of [CREATE DOMAIN](create-domain.md),
of rows 1, 50 and 2, 2000:

```sql
ALTER DOMAIN MONEY SET DEFAULT 10;
INSERT INTO LEDGER (ENTRY) VALUES (3);
ALTER DOMAIN MONEY DROP DEFAULT;
INSERT INTO LEDGER (ENTRY) VALUES (4);
ALTER DOMAIN MONEY ADD CONSTRAINT SMALL CHECK (VALUE < 1000);
ALTER DOMAIN MONEY ADD CONSTRAINT LARGE CHECK (VALUE < 5000);
INSERT INTO LEDGER VALUES (5, 6000);
SELECT * FROM LEDGER ORDER BY ENTRY;
```

```
OK
OK
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.SMALL ON LEDGER.AMOUNT of ANN.LEDGER is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.LARGE ON LEDGER.AMOUNT of ANN.LEDGER is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
        ENTRY |      AMOUNT
--------------+-------------
            1 |       50.00
            2 |     2000.00
            3 |       10.00
            4 |       10.00
(4 rows)
```

AMOUNT kept the default 10 when the domain's was dropped. SMALL was
refused, as the row of 2000 breaks it; LARGE, added, refused the row of
6000, naming the copy of it on AMOUNT.

Actions refused:

```sql
ALTER DOMAIN NOPE DROP DEFAULT;
ALTER DOMAIN MONEY DROP CONSTRAINT NOPE;
ALTER DOMAIN MONEY ADD CHECK (CAST(VALUE AS MONEY) > 0);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no domain ANN.NOPE [ISO/IEC 9075:1992 §11.22 SR1] (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.NOPE is not a constraint of domain ANN.MONEY [ISO/IEC 9075:1992 §11.26 SR3] (at character 35)
turborel: SQLCODE -103 (SQLSTATE 2A000): domain ANN.MONEY would be in usage by its own constraints [ISO/IEC 9075:1992 §11.21 SR4, §11.25 SR2] (at character 23)
```

## STANDARD

ISO/IEC 9075:1992 §11.22 to §11.26. SQL-86 has no domains.

## SEE ALSO

[CREATE DOMAIN](create-domain.md), [DROP DOMAIN](drop-domain.md),
[ALTER TABLE](alter-table.md)
