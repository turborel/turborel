# DROP VECTOR DISTANCE FUNCTION

[Statements](README.md)

## NAME

DROP VECTOR DISTANCE FUNCTION: drop a distance function *(extension)*

## SYNOPSIS

```
<drop distance function statement> ::=
    DROP VECTOR DISTANCE FUNCTION <schema qualified routine name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP VECTOR DISTANCE FUNCTION drops a function
[CREATE VECTOR DISTANCE FUNCTION](create-vector-distance-function.md)
defined, as [DROP FUNCTION](drop-routine.md) drops it, with the
privileges on it. VECTOR_DISTANCE no longer names it. It runs within
the session's transaction: ROLLBACK undoes it.

## RULES

1. The function exists, and its user owns its schema.
2. With RESTRICT, no view, constraint, trigger or routine calls
   VECTOR_DISTANCE by it. With CASCADE, what does is dropped too.

## PRIVILEGES

The user owns the function's schema.

## ERRORS

As [DROP ROUTINE](drop-routine.md)'s. Under RESTRICT, a definition
that names the function is SQLSTATE 42000 (SQLCODE -103).

## EXAMPLES

The example continues
[CREATE VECTOR DISTANCE FUNCTION](create-vector-distance-function.md)'s:

```sql
DROP VECTOR DISTANCE FUNCTION CHESSBOARD RESTRICT;
SELECT VECTOR_DISTANCE(P, '[0, 0]', CHESSBOARD) FROM POINTS;
```

```
OK
turborel: SQLCODE -301 (SQLSTATE 22000): unknown metric or distance function CHESSBOARD
```

## STANDARD

DROP VECTOR DISTANCE FUNCTION is an extension.

## SEE ALSO

[CREATE VECTOR DISTANCE FUNCTION](create-vector-distance-function.md),
[DROP ROUTINE](drop-routine.md)
