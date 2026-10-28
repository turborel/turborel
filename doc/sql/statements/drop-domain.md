# DROP DOMAIN

[Statements](README.md)

## NAME

DROP DOMAIN: drop a domain

## SYNOPSIS

```
<drop domain statement> ::= DROP DOMAIN <domain name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP DOMAIN drops a domain, and the USAGE privileges on it. RESTRICT
refuses the drop if a column is of the domain, or a view or a
constraint CASTs to it.

CASCADE drops it all the same. Each column of the domain keeps its data
type, and the domain's default, if it has none of its own; and each of
the domain's constraints is kept on it, as a constraint of its table.
The views, constraints and assertions that CAST to the domain are
dropped.

DROP DOMAIN runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The domain exists, and its user owns the domain's schema.
2. RESTRICT: no column is of the domain, and no view or constraint CASTs
   to it.
3. The transaction is not READ ONLY.

## PRIVILEGES

The domain's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the domain's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the domain MONEY of [CREATE DOMAIN](create-domain.md), DECIMAL(9,2)
DEFAULT 0 with the constraint NOT_NEGATIVE, and the table LEDGER (ENTRY
INTEGER, AMOUNT MONEY):

```sql
DROP DOMAIN MONEY RESTRICT;
DROP DOMAIN MONEY CASCADE;
INSERT INTO LEDGER (ENTRY) VALUES (1);
INSERT INTO LEDGER VALUES (2, -1);
SELECT * FROM LEDGER;
DROP DOMAIN NOPE CASCADE;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): domain ANN.MONEY is used by column ANN.LEDGER.AMOUNT, so it is dropped only with CASCADE [ISO/IEC 9075:1992 §11.27 SR2] (at character 12)
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.NOT_NEGATIVE ON LEDGER.AMOUNT of ANN.LEDGER is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
        ENTRY |      AMOUNT
--------------+-------------
            1 |        0.00
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no domain ANN.NOPE [ISO/IEC 9075:1992 §11.27 SR1] (at character 12)
```

AMOUNT kept the domain's default and its constraint.

## STANDARD

ISO/IEC 9075:1992 §11.27. SQL-86 has no domains.

## SEE ALSO

[CREATE DOMAIN](create-domain.md), [ALTER DOMAIN](alter-domain.md)
