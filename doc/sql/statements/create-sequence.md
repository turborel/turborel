# CREATE SEQUENCE

[Statements](README.md)

## NAME

CREATE SEQUENCE: define a sequence generator

## SYNOPSIS

```
<sequence generator definition> ::=
    CREATE SEQUENCE <sequence generator name> [ <sequence generator option>... ]

<sequence generator option> ::=
      AS <data type>
    | START WITH <signed numeric literal>
    | INCREMENT BY <signed numeric literal>
    | MAXVALUE <signed numeric literal> | NO MAXVALUE
    | MINVALUE <signed numeric literal> | NO MINVALUE
    | CYCLE | NO CYCLE

<next value expression> ::= NEXT VALUE FOR <sequence generator name>
```

## DESCRIPTION

A sequence generator gives out numbers in order. Each `NEXT VALUE FOR` takes
the next one, from its start, an increment at a time, within its bounds.

| Option | Default |
|---|---|
| AS | NUMERIC(18), scale 0 |
| INCREMENT BY | 1 |
| MAXVALUE | the type's greatest value, ascending, or -1, descending |
| MINVALUE | 1, ascending, or the type's least value, descending |
| START WITH | the minimum, ascending, or the maximum, descending |
| CYCLE | NO CYCLE |

A generator with a negative increment descends. Past its last value one of
NO CYCLE refuses to give more, and one of CYCLE starts again at its minimum,
or its maximum when it descends.

No transaction owns the generator's place. A value given out is never given
again, even when the transaction that took it rolls back, so a table filled
from a generator can have gaps. A crash can skip values too, up to 1024 of
them, but never repeats one. Each row of a query takes one value however many
times it names the generator.

NEXT VALUE FOR stands in the select list of a cursor's query, a SELECT INTO
or an INSERT's query, in an INSERT's VALUES, and in an UPDATE's SET. It is
refused elsewhere: in a CASE, a search condition, a set function, a grouped
or DISTINCT query, a subquery, a view, a constraint or a default. Its type is
the generator's.

CREATE SEQUENCE runs within the session's transaction, as other definitions
do. INFORMATION_SCHEMA.SEQUENCES shows the generators.

## RULES

1. No sequence generator of its name exists in its schema.
2. Its type is an exact numeric type of scale 0.
3. Each option is written at most once.
4. Each value is a whole number the type holds.
5. The increment is not 0, and the maximum is greater than the minimum.
6. The start is within the bounds.

## PRIVILEGES

Its schema's owner. NEXT VALUE FOR needs USAGE on the generator, which the
owner holds and GRANT gives.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken, or NEXT VALUE FOR is where it may not be |
| 2A000 | -104 | Its user does not own the schema, or NEXT VALUE FOR lacks USAGE |
| 2200H | -301 | NEXT VALUE FOR of a generator of NO CYCLE that has given its last value |

## EXAMPLES

```sql
CREATE SEQUENCE ORDER_NO START WITH 1000;
CREATE TABLE ORDERS (ID NUMERIC(18), ITEM VARCHAR(20));
INSERT INTO ORDERS VALUES (NEXT VALUE FOR ORDER_NO, 'pen'), (NEXT VALUE FOR ORDER_NO, 'ink');
SELECT ID, ITEM FROM ORDERS;
CREATE SEQUENCE DICE AS SMALLINT MINVALUE 1 MAXVALUE 3 CYCLE;
SELECT NEXT VALUE FOR DICE, NEXT VALUE FOR DICE FROM ORDERS;
SELECT ID FROM ORDERS WHERE ID < NEXT VALUE FOR ORDER_NO;
CREATE SEQUENCE BAD INCREMENT BY 0;
```

```
OK
OK
OK
                   ID | ITEM
----------------------+----------------------
                 1000 | pen
                 1001 | ink
(2 rows)
OK
         |
---------+---------
       1 |       1
       2 |       2
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): NEXT VALUE FOR is only directly in the select list of the whole query of a cursor, a SELECT INTO or an INSERT, not grouped nor DISTINCT, in an INSERT's VALUES or in an UPDATE's SET, and not in a CASE, a search condition, ORDER BY or a set function [ISO/IEC 9075-2:2023 §6.14 SR1, SR2] (at character 33)
turborel: SQLCODE -103 (SQLSTATE 2A000): the increment of a sequence generator is not 0 [ISO/IEC 9075-2:2023 §9.36 SR5, §9.37 SR5] (at character 17)
```

## STANDARD

ISO/IEC 9075-2:2023 §4.29, §6.14, §9.35, §9.36, §11.72, Feature T176. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sequence-generators).

## SEE ALSO

[ALTER SEQUENCE](alter-sequence.md), [DROP SEQUENCE](drop-sequence.md),
[GRANT](grant.md), [INSERT](insert.md)
