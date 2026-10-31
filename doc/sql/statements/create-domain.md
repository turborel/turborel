# CREATE DOMAIN

[Statements](README.md)

## NAME

CREATE DOMAIN: define a domain

## SYNOPSIS

```
<domain definition> ::=
    CREATE DOMAIN <domain name> [ AS ] <data type>
    [ DEFAULT <default option> ]
    [ <domain constraint>... ]

<domain constraint> ::=
    [ CONSTRAINT <constraint name> ] CHECK ( <search condition> ) [ <constraint attributes> ]
```

The `<default option>` and `<constraint attributes>` are those of
[CREATE TABLE](create-table.md). In a domain constraint, VALUE stands
for the value of a column of the domain. The data types are in [Data
Types](../02-data-types.md); a domain may also be of a type a plug-in
registers *(extension)*, written by its name and, in parentheses, one or
two unsigned integers it takes. A name alone here is that type, never
another domain.

## DESCRIPTION

CREATE DOMAIN defines a domain: a data type, with a default and
constraints, that columns may be defined as. A column of a domain takes
its data type; its default, unless the column has one of its own; and
each of its constraints, which holds of the column, VALUE the column's
value. A change to the domain, by ALTER DOMAIN, is a change to each
column of it.

CAST to a domain gives a value of its data type, which each of its
constraints is checked of: one false of it refuses the CAST.

CREATE DOMAIN runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The domain's schema exists, and its user owns it. A domain named
   without a schema is of the session's default schema.
2. No domain of the schema has the name.
3. The default is of the data type, as a column's default is.
4. The data type is not a domain; a name alone is a type a plug-in
   registered, with the parameters it takes *(extension)*.
5. A constraint's name is new in its schema, which exists, and which the
   domain's schema's owner owns *(extension)*. A name written without a
   schema is of the session's default schema; in a CREATE SCHEMA, of the
   schema being defined, and no other.
6. A constraint's condition follows the rules of a CHECK; the only
   column it names is VALUE.
7. No constraint CASTs to the domain, nor to a domain one of whose
   constraints does, however far.
8. The transaction is not READ ONLY.

## PRIVILEGES

The domain's owner holds USAGE on it, grantable. A column, or a CAST, of
a domain needs USAGE on it. A constraint's condition needs the
REFERENCES a CHECK needs.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist, or a constraint's does not |
| 01008 | 0 | Not an error: a warning, a default shorter than the BIT domain padded with 0 bits |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or a constraint's, or lacks a privilege a constraint needs |
| 25000 | -917 | The transaction is READ ONLY |
| 0A000 | -900 | A COLLATE clause |

A CAST to the domain of a value one of its constraints is false of fails
with SQLSTATE 23000, SQLCODE -201.

## EXAMPLES

```sql
CREATE DOMAIN MONEY AS DECIMAL(9,2) DEFAULT 0 CONSTRAINT NOT_NEGATIVE CHECK (VALUE >= 0);
CREATE TABLE LEDGER (ENTRY INTEGER, AMOUNT MONEY);
INSERT INTO LEDGER (ENTRY) VALUES (1);
INSERT INTO LEDGER VALUES (2, -5);
SELECT ENTRY, AMOUNT FROM LEDGER;
SELECT CAST(3 AS MONEY), CAST(-3 AS MONEY) FROM LEDGER;
```

```
OK
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.NOT_NEGATIVE ON LEDGER.AMOUNT of ANN.LEDGER is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
        ENTRY |      AMOUNT
--------------+-------------
            1 |        0.00
(1 row)

turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.NOT_NEGATIVE of domain ANN.MONEY is false for a value CAST to it [ISO/IEC 9075:1992 §6.10 GR13]
```

The domain's default gave AMOUNT its value; its constraint, a CHECK of
the column named `NOT_NEGATIVE ON LEDGER.AMOUNT`, refused -5, and the
CAST of -3.

## STANDARD

ISO/IEC 9075:1992 §11.21, and §6.10 GR13 for CAST to one. SQL-86 has no
domains.

## SEE ALSO

[ALTER DOMAIN](alter-domain.md), [DROP DOMAIN](drop-domain.md),
[CREATE TABLE](create-table.md), [GRANT](grant.md)
