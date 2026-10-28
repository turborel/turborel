# SET CONSTRAINTS

[Statements](README.md)

## NAME

SET CONSTRAINTS: set constraint modes

## SYNOPSIS

```
<set constraints mode statement> ::=
    SET CONSTRAINTS <constraint name list> { DEFERRED | IMMEDIATE }

<constraint name list> ::=
      ALL
    | <constraint name> [ { , <constraint name> }... ]
```

A `<constraint name>` written without a schema is of the session's
default schema.

## DESCRIPTION

SET CONSTRAINTS sets when deferrable constraints are checked: at the end
of each statement (IMMEDIATE), or as the transaction commits, or until
they are made immediate again (DEFERRED). ALL is every deferrable
constraint: of tables, of domains, and assertions.

A constraint is deferrable if its definition says DEFERRABLE; its mode
is first as its definition says, INITIALLY IMMEDIATE unless it says
INITIALLY DEFERRED. The modes SET CONSTRAINTS sets last until the
transaction ends. With no transaction active, it begins none, and its
modes are those of the next transaction.

Made IMMEDIATE, each constraint named is checked at once, of every change
the transaction made while it was deferred.

## RULES

1. Each constraint named exists.
2. Each constraint named is deferrable.
3. Of IMMEDIATE, in an active transaction, each constraint made
   immediate is checked; if one is not satisfied, the statement fails and
   the modes stay as they were.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A name is not a constraint's, or names one that is not deferrable |
| 23000 | -201 | Of IMMEDIATE, a constraint made immediate is not satisfied |

## EXAMPLES

A constraint deferred while a change breaks it for a time:

```sql
CREATE TABLE STOCK (
    ITEM INTEGER,
    QTY INTEGER CONSTRAINT IN_STOCK CHECK (QTY >= 0) DEFERRABLE,
    CONSTRAINT FIXED CHECK (ITEM > 0));
INSERT INTO STOCK VALUES (1, 5);
COMMIT;
SET CONSTRAINTS IN_STOCK DEFERRED;
UPDATE STOCK SET QTY = QTY - 8;
SET CONSTRAINTS IN_STOCK IMMEDIATE;
UPDATE STOCK SET QTY = QTY + 8;
SET CONSTRAINTS ALL IMMEDIATE;
COMMIT;
```

```
OK
OK
OK
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.IN_STOCK of ANN.STOCK is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
OK
OK
OK
```

The first SET CONSTRAINTS IMMEDIATE fails: QTY is -3. IN_STOCK stays
deferred, the transaction goes on, and once QTY is 5 again it is made
immediate.

Constraints that cannot be named:

```sql
SET CONSTRAINTS FIXED DEFERRED;
SET CONSTRAINTS NOPE IMMEDIATE;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): constraint ANN.FIXED is not DEFERRABLE [ISO/IEC 9075:1992 §14.2 SR2] (at character 16)
turborel: SQLCODE -103 (SQLSTATE 2A000): no constraint is named ANN.NOPE [ISO/IEC 9075:1992 §14.2 SR1] (at character 16)
```

## STANDARD

ISO/IEC 9075:1992 §14.2, and §10.6 for DEFERRABLE and INITIALLY, and
for checking a constraint as it is made immediate [§10.6 GR3]. Outside a
transaction, SET CONSTRAINTS sets the modes of the next [§14.2 GR1].
SQL-86 has no deferred constraints.

## SEE ALSO

[COMMIT](commit.md), [CREATE TABLE](create-table.md),
[CREATE ASSERTION](create-assertion.md), [CREATE DOMAIN](create-domain.md),
[Constraints](../../user/07-constraints.md)
