# DROP ASSERTION

[Statements](README.md)

## NAME

DROP ASSERTION: drop an assertion

## SYNOPSIS

```
<drop assertion statement> ::= DROP ASSERTION <constraint name>
```

No `<drop behavior>` is written: nothing depends on an assertion.

## DESCRIPTION

DROP ASSERTION drops an assertion; no row changes. A deferred
assertion dropped is not checked at commit.

DROP ASSERTION runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The assertion exists, and its user owns the assertion's schema.
2. The transaction is not READ ONLY.

## PRIVILEGES

The assertion's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | Anything is written after the name |
| 2A000 | -103 | There is no such assertion |
| 2A000 | -104 | Its user does not own the assertion's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE ASSERTION SMALL_LEDGER CHECK ((SELECT COUNT(*) FROM LEDGER) < 100);
COMMIT;
DROP ASSERTION SMALL_LEDGER;
DROP ASSERTION NOPE;
DROP ASSERTION SMALL_LEDGER CASCADE;
```

```
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no assertion ANN.NOPE [ISO/IEC 9075:1992 §11.35 SR2] (at character 15)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected end of statement, found CASCADE (at character 28)
```

## STANDARD

ISO/IEC 9075:1992 §11.35. SQL-86 has no assertions.

## SEE ALSO

[CREATE ASSERTION](create-assertion.md)
