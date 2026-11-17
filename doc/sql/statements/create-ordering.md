# CREATE ORDERING

[Statements](README.md)

## NAME

CREATE ORDERING: say how values of a user-defined type compare

## SYNOPSIS

```
<user-defined ordering definition> ::=
    CREATE ORDERING FOR <user-defined type name> <ordering form>

<ordering form> ::=
      EQUALS ONLY BY <ordering category>
    | ORDER FULL BY <ordering category>

<ordering category> ::=
      RELATIVE WITH <specific routine designator>
    | MAP WITH <specific routine designator>
    | STATE [ <specific name> ]
```

## DESCRIPTION

A structured type has no ordering until CREATE ORDERING gives it one, so
its values compare by nothing. EQUALS ONLY lets them be compared by `=`
and `<>` alone. ORDER FULL lets them be compared by each comparison
operator, sorted and grouped.

MAP compares values by the values its function maps them to, a
predefined type's. STATE compares them attribute by attribute, and is
EQUALS ONLY. RELATIVE compares two values by its function's result,
negative, zero or positive. A sort or grouping by a RELATIVE ordering is
refused.

Values of a type and its subtypes compare by the ordering of the
family's maximal supertype. A subtype's own ordering has its
supertype's form, and is by MAP if the supertype's is. A distinct type
is given an ordering by its definition, its source's, which DROP
ORDERING may drop and CREATE ORDERING replace by MAP or RELATIVE.

CREATE ORDERING runs within the session's transaction, as other
definitions do. INFORMATION_SCHEMA.USER_DEFINED_TYPES shows each type's
ordering.

## RULES

1. The type exists, its user owns it, and it has no ordering.
2. A subtype's ordering has its direct supertype's form, and is by MAP
   if the supertype's is.
3. STATE is EQUALS ONLY, of a structured type whose attributes compare
   for equality.
4. A MAP function is DETERMINISTIC, reads or modifies no SQL data, and
   takes one parameter of the type, and returns a predefined type.
5. A RELATIVE function is DETERMINISTIC, reads or modifies no SQL data,
   takes two parameters of the type and returns INTEGER.

## PRIVILEGES

The owner of the type, who needs EXECUTE on the function.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken, or values of a type of no ordering are compared |
| 2A000 | -104 | Its user does not own the type, or lacks EXECUTE on the function |
| 0A000 | -900 | A sort or grouping by a RELATIVE ordering |

## EXAMPLES

```sql
CREATE TYPE DIMS AS (W INTEGER, H INTEGER) FINAL;
CREATE TABLE BOXES (K INTEGER, D DIMS);
INSERT INTO BOXES VALUES (1, NEW DIMS().W(2).H(5)), (2, NEW DIMS().W(3).H(3)), (3, NEW DIMS().W(1).H(9));
SELECT K FROM BOXES ORDER BY D;
```

```
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): ORDER BY compares values of ANN.DIMS, which has no ordering [ISO/IEC 9075-2:2023 §9.14 SR3]
```

```sql
CREATE FUNCTION AREA (D DIMS) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN D.W * D.H;
CREATE ORDERING FOR DIMS ORDER FULL BY MAP WITH FUNCTION AREA (DIMS);
SELECT K FROM BOXES ORDER BY D;
SELECT COUNT(*) FROM BOXES WHERE D > NEW DIMS().W(3).H(3);
```

```
OK
OK
            K
--------------
            2
            3
            1
(3 rows)

----------------------
                    1
(1 row)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.65, §8.2, §9.12, §9.14, Feature S024. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[DROP ORDERING](drop-ordering.md), [CREATE TYPE](create-type.md)
