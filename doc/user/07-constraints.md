# Constraints

[Contents](README.md)

A constraint is a rule the rows must keep; a change that would break one
is refused, SQLSTATE 23000, the statement undone. Constraints are
written with a table ([CREATE TABLE](../sql/statements/create-table.md)),
added and dropped by [ALTER TABLE](../sql/statements/alter-table.md).

The examples are of the tables of [Getting Started](02-getting-started.md),
and of teams and their members:

```sql
CREATE TABLE TEAMS (ID INTEGER PRIMARY KEY, LEAD INTEGER NOT NULL);
CREATE TABLE MEMBERS (ID INTEGER PRIMARY KEY, TEAM INTEGER NOT NULL,
    CONSTRAINT IN_TEAM FOREIGN KEY (TEAM) REFERENCES TEAMS DEFERRABLE INITIALLY DEFERRED);
COMMIT;
```

```
OK
OK
OK
```

## NOT NULL and UNIQUE

NOT NULL refuses the null value. UNIQUE refuses two rows of the same
values in its columns; rows with a null in them are never alike.

```sql
INSERT INTO MEMBERS (ID) VALUES (13);
```

```
turborel: SQLCODE -201 (SQLSTATE 23000): column TEAM of ANN.MEMBERS is NOT NULL [X3.135-1986 §6.3 SR3, GR1]
```

## Primary and foreign keys

A PRIMARY KEY is UNIQUE and NOT NULL: one row of each key, the table's
way to name a row. A FOREIGN KEY, or REFERENCES, refuses a row whose
values match no row of the table it references, and says what is done
to the rows that refer to one deleted or changed: refused, the default;
CASCADE, deleted or changed with it; SET NULL or SET DEFAULT.

```sql
INSERT INTO TEAMS VALUES (1, 10);
INSERT INTO TEAMS VALUES (1, 11);
```

```
OK
turborel: SQLCODE -201 (SQLSTATE 23000): PRIMARY KEY ANN.TEAMS_PRIMARY1 (ID) of ANN.TEAMS would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
```

TEAMS has a team 1 already when the second is inserted. A constraint
not named is given a name, as `TEAMS_PRIMARY1`. The foreign key IN_TEAM
is checked at COMMIT ([Deferred checking](#deferred-checking)).

## CHECK constraints

A CHECK refuses a row its condition is false of; one it is unknown of,
as of a null, passes. Its condition may read other tables, through a
subquery.

```sql
UPDATE EMPLOYEES SET SALARY = -1 WHERE ID = 3;
```

```
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.EMPLOYEES_CHECK1 of ANN.EMPLOYEES is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
```

## Assertions

An assertion is a constraint of a schema, on no one table: its condition
holds of the database whole, checked after each statement that changes
a table it reads ([CREATE ASSERTION](../sql/statements/create-assertion.md)).

## Deferred checking

A constraint is checked at the end of each statement, unless it is
deferred: then at COMMIT. A constraint DEFERRABLE INITIALLY DEFERRED is
deferred in each transaction; [SET
CONSTRAINTS](../sql/statements/set-constraints.md) defers one DEFERRABLE,
or makes one deferred IMMEDIATE. So rows that refer to each other may be
inserted in either order:

```sql
INSERT INTO MEMBERS VALUES (20, 2);
INSERT INTO TEAMS VALUES (2, 20);
COMMIT;
INSERT INTO MEMBERS VALUES (21, 3);
COMMIT;
SET CONSTRAINTS IN_TEAM IMMEDIATE;
INSERT INTO MEMBERS VALUES (22, 4);
```

```
OK
OK
OK
OK
turborel: SQLCODE -913 (SQLSTATE 40002): the transaction is rolled back: a row of ANN.MEMBERS references no row of ANN.TEAMS: FOREIGN KEY ANN.IN_TEAM [ISO/IEC 9075:1992 §11.8 GR2] [ISO/IEC 9075:1992 §14.3 GR5a]
OK
turborel: SQLCODE -201 (SQLSTATE 23000): a row of ANN.MEMBERS references no row of ANN.TEAMS: FOREIGN KEY ANN.IN_TEAM [ISO/IEC 9075:1992 §11.8 GR2]
```

Member 20 came before its team, and was checked as the transaction
committed, team 1 with it. Member 21's team 3 never came: the COMMIT
failed, and rolled the transaction back. Made IMMEDIATE, the constraint
refused member 22 at once.
