# DROP ORDERING

[Statements](README.md)

## NAME

DROP ORDERING: drop a user-defined type's ordering

## SYNOPSIS

```
<drop user-defined ordering statement> ::=
    DROP ORDERING FOR <user-defined type name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP ORDERING drops a user-defined type's ordering, and leaves its
function, so the type's values compare by nothing again, until CREATE
ORDERING gives it another. RESTRICT
refuses the drop while a view, a constraint, an assertion, a generated
column or a routine compares, sorts or groups values of the type.
CASCADE drops those first.

DROP ORDERING runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The type has an ordering, and its user owns the type.
2. RESTRICT: nothing depends on the ordering.

## PRIVILEGES

The owner of the type.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | Its user does not own the type |

## EXAMPLES

```sql
CREATE TYPE DIMS AS (W INTEGER, H INTEGER) FINAL;
CREATE ORDERING FOR DIMS EQUALS ONLY BY STATE;
CREATE VIEW SAME (E) AS SELECT NEW DIMS().W(1).H(1) = NEW DIMS().W(1).H(1) FROM (VALUES (1)) AS T (N);
DROP ORDERING FOR DIMS RESTRICT;
```

```
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 42000): view ANN.SAME binds only with the ordering of ANN.DIMS, so it is dropped only with CASCADE [ISO/IEC 9075-2:2023 §11.66 SR4] (at character 18)
```

```sql
DROP ORDERING FOR DIMS CASCADE;
SELECT NEW DIMS() = NEW DIMS() FROM (VALUES (1)) AS T (N);
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 42000): the comparison compares values of ANN.DIMS, which has no ordering [ISO/IEC 9075-2:2023 §9.10 SR3] (at character 7)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.66, Feature S024. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[CREATE ORDERING](create-ordering.md)
