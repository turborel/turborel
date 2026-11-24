# DROP CAST

[Statements](README.md)

## NAME

DROP CAST: drop a user-defined cast

## SYNOPSIS

```
<drop user-defined cast statement> ::=
    DROP CAST ( <source data type> AS <target data type> ) <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP CAST drops a user-defined cast, and leaves its function. RESTRICT
refuses the drop while a view, a constraint, an assertion, a generated
column or a routine invokes the cast. CASCADE drops those first, as
their own drops with CASCADE do.

DROP CAST runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The cast exists, and its user owns the cast's user-defined type.
2. RESTRICT: nothing invokes the cast.

## PRIVILEGES

The owner of the cast's user-defined type.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | Its user does not own the type |

## EXAMPLES

```sql
CREATE TYPE TEMP AS (DEGREES INTEGER) FINAL;
CREATE FUNCTION TEMP_TO_INT (T TEMP) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN T.DEGREES;
CREATE CAST (TEMP AS INTEGER) WITH FUNCTION TEMP_TO_INT (TEMP);
CREATE VIEW HOT (C) AS SELECT CAST(NEW TEMP().DEGREES(40) AS INTEGER) FROM (VALUES (1)) AS T (N);
DROP CAST (TEMP AS INTEGER) RESTRICT;
```

```
OK
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 42000): view ANN.HOT binds only with the cast from ANN.TEMP to INTEGER, so it is dropped only with CASCADE [ISO/IEC 9075-2:2023 §11.64 SR7] (at character 5)
```

```sql
DROP CAST (TEMP AS INTEGER) CASCADE;
SELECT CAST(NEW TEMP().DEGREES(40) AS INTEGER) FROM (VALUES (1)) AS T (N);
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 42000): no user-defined cast takes ANN.TEMP to INTEGER, nor is INTEGER a supertype of it [ISO/IEC 9075-2:2023 §6.13 SR12] (at character 7)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.64, Feature S211. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[CREATE CAST](create-cast.md)
