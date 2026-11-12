# DROP SEQUENCE

[Statements](README.md)

## NAME

DROP SEQUENCE: drop a sequence generator

## SYNOPSIS

```
<drop sequence generator statement> ::=
    DROP SEQUENCE <sequence generator name> { RESTRICT | CASCADE }
```

## DESCRIPTION

DROP SEQUENCE drops a generator, and every USAGE granted on it. No view,
constraint or default can name a generator, so nothing depends on one, and
RESTRICT and CASCADE do the same. DROP SCHEMA with CASCADE drops the schema's
generators too.

A generator made again with the same name starts afresh.

DROP SEQUENCE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The generator exists.

## PRIVILEGES

The owner of the generator's schema.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | No such generator |
| 2A000 | -104 | Its user does not own the generator's schema |

## EXAMPLES

```sql
CREATE SEQUENCE S;
DROP SEQUENCE S RESTRICT;
SELECT NEXT VALUE FOR S FROM (VALUES (0)) AS X (Z);
DROP SEQUENCE S CASCADE;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no sequence generator ANN.S [ISO/IEC 9075-2:2023 §6.14 SR3] (at character 7)
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no sequence generator ANN.S [ISO/IEC 9075-2:2023 §11.74 SR2] (at character 14)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.74, Feature T176. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sequence-generators).

## SEE ALSO

[CREATE SEQUENCE](create-sequence.md), [ALTER SEQUENCE](alter-sequence.md),
[DROP SCHEMA](drop-schema.md)
