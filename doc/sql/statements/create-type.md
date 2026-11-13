# CREATE TYPE

[Statements](README.md)

## NAME

CREATE TYPE: define a distinct type

## SYNOPSIS

```
<user-defined type definition> ::=
    CREATE TYPE <user-defined type name> AS <representation>
    [ <type option>... ]

<representation> ::= <predefined type> | <array type>

<type option> ::=
      FINAL
    | CAST ( SOURCE AS DISTINCT ) WITH <function name>
    | CAST ( DISTINCT AS SOURCE ) WITH <function name>

<routine invocation> ::= [ <schema name> . ] <function name> ( <value expression> )
```

## DESCRIPTION

A distinct type is a type of its own made from its source type. Its
values are those of the source, but they compare only with values of the
same distinct type, so values of two types of one source, such as prices
and weights, cannot be compared by mistake.

The source is a predefined type or an array type. An exact numeric
source is kept as NUMERIC of its precision and scale, so INTEGER is
NUMERIC(10,0), and an approximate one as FLOAT of its precision.

A value is assigned to a column of the type if it is of the type, or of
a type that is assigned to the source, such as an integer to a type of
DECIMAL. A value of the type is assigned only to a site of exactly the
source type. CAST takes the same paths and no other. The values compare,
order and group as the source's do, so a column of the type may be a key
or be indexed. A type whose source is an array type compares with
nothing. No operator but comparison takes the type's values. Arithmetic,
SUM and the string functions are refused, and the values are taken out
first by the function to the source.

The definition makes two functions. The first makes a value of the type
from one of its source and is named for the type, or as `CAST (SOURCE AS
DISTINCT) WITH` names it. The second gives a value's source value and is
named for the source type, NUMERIC, CHARACTER and so on, or as `CAST
(DISTINCT AS SOURCE) WITH` names it. A function named by a key word, as
NUMERIC is, is invoked delimited, `"NUMERIC"(PRICE)`. A name not
qualified is found in the session's default schema, or in the schema
statement's own. Two types may name their functions alike. Of functions
of one name, the argument's type chooses, so two types of NUMERIC and
INTEGER sources may each name their function to the source AMOUNT. Their
specific names, which GRANT and REVOKE may name, are the type's name and
_TO_DISTINCT or _TO_SOURCE.

A client reads and writes a value of the type as a value of its source,
and DESCRIBE describes it as TYPE 17, with the type's name.

CREATE TYPE runs within the session's transaction, as other definitions
do. INFORMATION_SCHEMA.USER_DEFINED_TYPES shows the types, and ROUTINES
the functions.

## RULES

1. The schema exists, and no type or domain of the schema has its name.
2. The source is a predefined type or an array type, and no row type,
   distinct type or domain.
3. Each option is written at most once. NOT FINAL, INSTANTIABLE, UNDER,
   REF and the CASTs to REF and TYPE, which are of structured types, are
   refused.
4. No function of the schema of the name of one of its functions takes a
   parameter of the same kind of type.
5. A column of the type defaults to NULL alone, or to ARRAY[] if the
   source is an array type.

## PRIVILEGES

Its schema's owner, who holds USAGE on the type and EXECUTE on its
functions with grant option. Naming the type needs USAGE, and invoking a
function, or a CAST to or from the type, needs EXECUTE on the function.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -103 | A rule above is broken, or a value of the type meets an operator or a type it is not taken by |
| 2A000 | -104 | Its user does not own the schema, or lacks USAGE on the type or EXECUTE on a function |
| 0A000 | -900 | The definition is of a structured type or has methods |

## EXAMPLES

```sql
CREATE TYPE MONEY AS DECIMAL(10,2);
CREATE TYPE GRAMS AS INTEGER CAST (DISTINCT AS SOURCE) WITH WEIGHT;
CREATE TABLE PARCELS (ID INTEGER, PRICE MONEY, MASS GRAMS);
INSERT INTO PARCELS VALUES (1, 12.5, 400), (2, 8, 250);
SELECT ID, PRICE FROM PARCELS WHERE PRICE > MONEY(10);
SELECT SUM("NUMERIC"(PRICE)), MAX(MASS) FROM PARCELS;
SELECT WEIGHT(MASS) * 2 FROM PARCELS ORDER BY 1;
SELECT ID FROM PARCELS WHERE PRICE > 10;
```

```
OK
OK
OK
OK
           ID |        PRICE
--------------+--------------
            1 |        12.50
(1 row)
                      |
----------------------+--------------
                20.50 |          400
(1 row)

----------------------
                  500
                  800
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.MONEY and NUMERIC(2,0) are not comparable [ISO/IEC 9075:1992 §8.2 SR1, SR3] (at character 29)
```

```sql
SELECT ID FROM PARCELS WHERE PRICE = MASS;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.MONEY and ANN.GRAMS are not comparable [ISO/IEC 9075:1992 §8.2 SR1, SR3] (at character 29)
```

```sql
CREATE TYPE BAD AS MONEY;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): a distinct type's representation is a <predefined type> or a <collection type>, not MONEY [ISO/IEC 9075-2:2023 §11.51 SR5a] (at character 19)
```

## STANDARD

ISO/IEC 9075-2:2023 §4.9, §11.51, §9.18, §10.4, Features S011, S028,
S401. See [CONFORMANCE.md](../../../CONFORMANCE.md#distinct-types).

## SEE ALSO

[DROP TYPE](drop-type.md), [GRANT](grant.md), [REVOKE](revoke.md),
[Data Types](../02-data-types.md#distinct-types)
