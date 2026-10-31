# EXECUTE

[Statements](README.md)

## NAME

EXECUTE: run a statement prepared *(in a program)*

## SYNOPSIS

```
<execute statement> ::=
    EXECUTE <SQL statement name>
    [ <result using clause> ]
    [ <parameter using clause> ]

<result using clause> ::=
      INTO <target specification> [ { , <target specification> }... ]
    | INTO SQL DESCRIPTOR <descriptor name>

<parameter using clause> ::=
      USING <target specification> [ { , <target specification> }... ]
    | USING SQL DESCRIPTOR <descriptor name>

<descriptor name> ::= [ GLOBAL | LOCAL ] <simple value specification>
```

The `<SQL statement name>` is [PREPARE](prepare.md)'s, the `<target
specification>` [FETCH](fetch.md)'s.

## DESCRIPTION

EXECUTE runs the statement prepared by the name, with values for its
dynamic parameters, the `?`s, in order: the parameters of the USING
clause, each with its indicator if written, a negative one giving the
null value; or the items of an SQL descriptor area, each its DATA, or
the null value if its INDICATOR is negative
([SET DESCRIPTOR](set-descriptor.md)). Each value is cast to its
parameter's type, as CAST does.

A query prepared that is a single SELECT, with no ORDER BY, UNION or FOR
clause, is run as SELECT ... INTO runs one: its one row is put where the
INTO clause says, into the targets, as FETCH assigns them, or into the
items of a descriptor area, each value cast to its item's type, its
INDICATOR 0 and DATA the value, or INDICATOR -1 of the null value; and
RETURNED_LENGTH and RETURNED_OCTET_LENGTH of a CHARACTER VARYING or BIT
VARYING value. No row is no data, SQLSTATE 02000; more than one, an
exception. Any other query is read by a cursor
([ALLOCATE CURSOR](allocate-cursor.md)), not executed.

Any other statement runs as it would run written there. EXECUTE is a
statement of its own: the diagnostics area names EXECUTE, and the
statement prepared as its DYNAMIC_FUNCTION, `DYNAMIC UPDATE CURSOR` and
`DYNAMIC DELETE CURSOR` of a positioned UPDATE and DELETE; it begins a
transaction if the statement prepared does.

EXECUTE runs only in a program that embeds turborel, as
[PREPARE](prepare.md) does.

## RULES

1. A statement of the name is prepared.
2. A query executed is a single SELECT, and has an INTO clause.
3. A statement of dynamic parameters has a USING clause; of none, no
   USING clause.
4. The USING clause gives one value for each parameter, of a type cast
   to the parameter's; a descriptor area's COUNT is from 0 to its
   occurrences, and its items each describe a valid type, DATA a value
   of it if INDICATOR is not negative.
5. The INTO clause has a target for each column, or a descriptor area
   whose COUNT is the number of columns, each item of a type the column's
   values are cast to.
6. The INTO clause comes first, the USING clause after.

## PRIVILEGES

Those the statement prepared needs.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 26000 | -921 | No statement of the name is prepared (rule 1) |
| 07003 | -921 | A query that is not a single SELECT (rule 2) |
| 07007 | -921 | A single SELECT and no INTO clause (rule 2) |
| 07004 | -921 | Dynamic parameters and no USING clause (rule 3) |
| 07001 | -921 | The USING clause does not give one valid value for each parameter (rules 3, 4) |
| 07002 | -921 | The INTO clause does not match the columns (rule 5) |
| 07008 | -921 | A descriptor area's COUNT is less than 0 or more than its occurrences (rules 4, 5) |
| 07006 | -921 | A value's type is not cast to its parameter's or item's (rules 4, 5) |
| 33000 | -921 | No descriptor area of the name is allocated |
| 2A000 | -103 | INTO in the USING clause, or USING in the INTO clause (rule 6) |
| 21000 | -811 | A single SELECT gives more than one row |
| 02000 | 100 | Not an error: no data, the single SELECT gave no row |

## EXAMPLES

In a program, of the statements [PREPARE](prepare.md) prepares,
RAISE_ONE and the GLOBAL statement NAME_OF, and the parameters it
declares:

```rust
s.execute("EXECUTE RAISE_ONE USING RAISE, MY_ID", &mut p)?;    // Ada's SALARY 7300.00
p.set("MY_ID", Value::Exact(Decimal::new(3, 0)));
s.execute("EXECUTE GLOBAL STMT INTO MY_NAME INDICATOR NAME_IND USING MY_ID", &mut p)?;  // NAME_IND = -1
p.set("MY_ID", Value::Exact(Decimal::new(9, 0)));
s.execute("EXECUTE GLOBAL STMT INTO MY_NAME INDICATOR NAME_IND USING MY_ID", &mut p)?;  // no data
```

Refused, MANY prepared of `SELECT NAME FROM STAFF WHERE ID >= ?`,
ORDERED of `SELECT NAME FROM STAFF ORDER BY NAME`, and MY_ID 1:

```
EXECUTE GONE                                               -921 (26000): no statement GONE is prepared [ISO/IEC 9075:1992 §17.10 GR1]
EXECUTE RAISE_ONE                                          -921 (07004): the statement has dynamic parameters, and no <using clause> [ISO/IEC 9075:1992 §17.10 GR4]
EXECUTE RAISE_ONE USING RAISE                              -921 (07001): 1 values for 2 <dynamic parameter specification>s [ISO/IEC 9075:1992 §17.9 GR5a]
EXECUTE GLOBAL STMT USING MY_ID                            -921 (07007): a <dynamic single row select statement> needs a <result using clause> [ISO/IEC 9075:1992 §17.10 GR5]
EXECUTE MANY INTO MY_NAME INDICATOR NAME_IND USING MY_ID   -811 (21000): the result has more than one row [ISO/IEC 9075:1992 §13.5 GR2a]
EXECUTE ORDERED INTO MY_NAME INDICATOR NAME_IND            -921 (07003): a <cursor specification> of more than a <query specification> is opened, not executed [ISO/IEC 9075:1992 §17.10 GR2]
```

Of a descriptor area, see [DESCRIBE](describe.md).

## STANDARD

ISO/IEC 9075:1992 §17.10, of the `<using clause>` of §17.9. SQL-92 makes
it no `<directly executable statement>` [§20.1]. SQL-86 has no dynamic
SQL.

## SEE ALSO

[PREPARE](prepare.md), [EXECUTE IMMEDIATE](execute-immediate.md),
[DESCRIBE](describe.md), [GET DESCRIPTOR](get-descriptor.md),
[SET DESCRIPTOR](set-descriptor.md), [SELECT](select.md)
