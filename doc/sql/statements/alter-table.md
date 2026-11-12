# ALTER TABLE

[Statements](README.md)

## NAME

ALTER TABLE: change a base table

## SYNOPSIS

```
<alter table statement> ::=
    ALTER TABLE <table name> <alter table action>

<alter table action> ::=
      ADD [ COLUMN ] <column definition>
    | ALTER [ COLUMN ] <column name> <alter column action>
    | DROP [ COLUMN ] <column name> <drop behavior>
    | ADD <table constraint definition>
    | DROP CONSTRAINT <constraint name> <drop behavior>
    | SET ( <table option> [ { , <table option> }... ] )
    | USING <engine name> [ WITH ( <table option> [ { , <table option> }... ] ) ]
    | RESEAL

<alter column action> ::=
      SET DEFAULT <default option>
    | DROP DEFAULT
    | <alter identity column option>...
    | SET GENERATED { ALWAYS | BY DEFAULT } [ <alter identity column option>... ]
    | DROP IDENTITY
    | DROP EXPRESSION

<alter identity column option> ::=
      RESTART [ WITH <signed numeric literal> ]
    | SET INCREMENT BY <signed numeric literal>
    | SET MAXVALUE <signed numeric literal> | SET NO MAXVALUE
    | SET MINVALUE <signed numeric literal> | SET NO MINVALUE
    | SET CYCLE | SET NO CYCLE

<drop behavior> ::= CASCADE | RESTRICT
```

The `<column definition>`, `<default option>`, `<table constraint
definition>` and `<table option>` are those of
[CREATE TABLE](create-table.md). One statement takes one action.

## DESCRIPTION

ALTER TABLE changes the definition of a base table.

**ADD COLUMN** adds a column, last. Each row has its default in it, or,
of an identity column, its generator's next value, or, of a generated
column, its expression's value. It
takes the constraints written in its definition, and those of its
domain. Each privilege held on the whole table, of SELECT, INSERT,
UPDATE or REFERENCES, is held on the new column too, by the same
grantee, with the same grant option.

**ALTER COLUMN** sets the column's default, or drops it. No row changes;
the new default is taken by the rows inserted later.

Of an identity column, ALTER COLUMN changes its generator's options as
[ALTER SEQUENCE](alter-sequence.md) does, and SET GENERATED changes
whether its values are always generated. RESTART starts the generator
again at its start, or at the value written. A changed increment goes on
from the generator's current base value. DROP IDENTITY makes it a column
that takes values of its own, of no default, and drops its generator. Its
NOT NULL constraint stays. Of a generated column, DROP EXPRESSION makes it
a column that takes values of its own, keeping those it holds. No row
changes.

**DROP COLUMN** removes the column and its values; the columns after it
move up one place. A table constraint that reads that column alone goes
with it. RESTRICT refuses the drop if a view, an assertion, a foreign
key of another table, or a constraint that reads other columns too
refers to the column; CASCADE drops them with it. RESTRICT refuses it too
if a generated column's expression references the column; CASCADE first
drops each such generated column, as DROP COLUMN CASCADE would. If the drop
then fails, the transaction can only roll back. The privileges on the
column go.

**ADD CONSTRAINT** adds a table constraint, checked of the rows there
now; a row that breaks it refuses it.

**DROP CONSTRAINT** drops a constraint of the table, its name written
without a schema being of the session's default schema. RESTRICT refuses
the drop of a UNIQUE or PRIMARY KEY constraint that a foreign key
references; CASCADE drops that foreign key too.

**SET and USING** *(extension)* change how the table is stored: SET
changes the options of its engine, DEFAULT an option's default; USING
moves its rows to another engine, or to the same one made again, with
the options WITH gives. INFORMATION_SCHEMA.TABLE_OPTIONS shows them. An
option the engine can take in place changes no row; any other change
copies the rows to storage made anew.

**RESEAL** *(extension)* copies the table's rows to storage made anew,
as it is, and makes its indexes anew. Rows written before its option
ENCODINGS changed keep the encodings, and the key, they were written
with; copied, they pass through those ENCODINGS names now. Once the
transaction commits and a checkpoint follows, nothing of the table is
read with a key it named before, which may then be retired
([Encryption](../../operator/07-encryption.md#retiring-a-key)). RESEAL is
a non-reserved word: elsewhere, an identifier.

A view sees the columns its tables had when it was defined: a column
added is not one of its.

ALTER TABLE runs within the session's transaction: ROLLBACK undoes it.
Until the transaction ends, others read the table as it was; one that
writes it is refused (SQLSTATE TB911), or, after `SET LOCKS.WAIT =
'ON'` ([SET](set-setting.md)), waits for it.

## RULES

1. The table is a base table, not a view, and its user owns the table's
   schema.
2. ADD COLUMN: no column of the table has the name. If its definition is
   NOT NULL and the table has rows, the column has a default.
3. ALTER COLUMN and DROP COLUMN: the table has the column. DROP DEFAULT:
   the column has a default of its own. SET DEFAULT and DROP DEFAULT are
   not of an identity column, and SET DEFAULT is not of a generated
   column. SET GENERATED, the generator's options and DROP IDENTITY are of
   an identity column alone, and DROP EXPRESSION of a generated column
   alone. The options follow the rules of [ALTER SEQUENCE](alter-sequence.md).
   ADD COLUMN adds no second identity column.
4. DROP COLUMN: the column is not the table's only one, nor one a table
   option names, such as COLUMNSTORE.ORDER *(extension)*: the option is
   changed first.
5. DROP CONSTRAINT: the table has the constraint. A constraint a column
   has from its domain is changed by [ALTER DOMAIN](alter-domain.md).
6. A constraint added follows the rules of [CREATE TABLE](create-table.md).
7. SET and USING: the engine is registered, and takes each option, of a
   value it accepts; an option of columns names the table's. A temporary table names no engine or options,
   and is not resealed.
8. A temporary table is altered only while no other transaction has a
   session's rows of it open, and by a transaction that sees each
   session's rows: one begun after the last commit that wrote them. ADD
   and DROP COLUMN change each session's rows, its own and another's,
   the column added taking its default in each. Any other action changes
   no row and is every session's at once. The table's constraints, a
   constraint added among them, hold of each session's rows, read as
   that session sees the other temporary tables: its own rows of them.
9. No cursor of the transaction reads the table.
10. The transaction is not READ ONLY.

## PRIVILEGES

The table's schema is its user's. A column added, or a constraint,
needs the USAGE and REFERENCES that [CREATE TABLE](create-table.md)
says.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The action is not written as the synopsis shows |
| 2A000 | -103 | A rule above is broken |
| 3F000 | -103 | The schema of a constraint added does not exist |
| 2A000 | -104 | Its user does not own the table's schema, or lacks a privilege a column or constraint needs |
| 23000 | -201 | A row breaks the constraint added; a NOT NULL column added has no default, and the table has rows |
| 25000 | -917 | The transaction is READ ONLY |
| 24000 | -501 | A cursor of the transaction reads the table |
| TB911 | -911 | Another transaction has a session's rows of a temporary table open, or committed rows of it after the transaction began; another transaction holds a lock on the table |

## EXAMPLES

Of a table STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), SALARY
DECIMAL(9,2)) of rows 1, Ada, 7200 and 2, Grace, 8100:

```sql
ALTER TABLE STAFF ADD COLUMN DEPT CHAR(3) DEFAULT 'ENG';
ALTER TABLE STAFF ALTER COLUMN DEPT SET DEFAULT 'OPS';
INSERT INTO STAFF (ID, NAME, SALARY) VALUES (3, 'Edsger', 6900);
ALTER TABLE STAFF ADD CONSTRAINT PAID CHECK (SALARY > 7000);
ALTER TABLE STAFF ADD CONSTRAINT PAID CHECK (SALARY > 0);
SELECT * FROM STAFF ORDER BY ID;
```

```
OK
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.PAID of ANN.STAFF is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
OK
           ID | NAME                           |      SALARY | DEPT
--------------+--------------------------------+-------------+------
            1 | Ada                            |     7200.00 | ENG
            2 | Grace                          |     8100.00 | ENG
            3 | Edsger                         |     6900.00 | OPS
(3 rows)
```

The rows there took the default DEPT had when it was added; the row
inserted after, the default set later. The first PAID was refused, as
Edsger's row breaks it.

PAID reads SALARY alone, so it goes with the column; a view that reads
SALARY does not:

```sql
CREATE VIEW PAY AS SELECT NAME, SALARY FROM STAFF;
ALTER TABLE STAFF DROP COLUMN SALARY RESTRICT;
ALTER TABLE STAFF DROP COLUMN SALARY CASCADE;
SELECT * FROM PAY;
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): view ANN.PAY refers to ANN.STAFF.SALARY, so it is dropped only with CASCADE [ISO/IEC 9075:1992 §11.15 SR4]
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.PAY is not defined [X3.135-1986 §5.4 SR5] (at character 14)
```

An identity column added to a table of rows numbers them, and is then
restarted:

```sql
ALTER TABLE STAFF ADD COLUMN BADGE INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 500);
ALTER TABLE STAFF ALTER COLUMN BADGE RESTART WITH 900;
INSERT INTO STAFF (ID, NAME) VALUES (4, 'Barbara');
SELECT ID, BADGE FROM STAFF ORDER BY ID;
```

```
OK
OK
OK
           ID |        BADGE
--------------+--------------
            1 |          500
            2 |          501
            3 |          502
            4 |          900
(4 rows)
```

Its storage *(extension)*:

```sql
ALTER TABLE STAFF SET (STATISTICS.BUCKETS = 50);
ALTER TABLE STAFF USING ROWSTORE WITH (OUT_OF_LINE = 200);
SELECT OPTION_NAME, OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS
    WHERE TABLE_NAME = 'STAFF' AND OPTION_NAME IN ('OUT_OF_LINE', 'STATISTICS.BUCKETS') ORDER BY 1;
```

```
OK
OK
 OPTION_NAME                                                  | OPTION_VALUE
--------------------------------------------------------------+--------------------------------------------------------------
 OUT_OF_LINE                                                  | 200
 STATISTICS.BUCKETS                                           | 50
(2 rows)
```

Its rows sealed again *(extension)*, through the encodings its
ENCODINGS names now:

```sql
ALTER TABLE STAFF RESEAL;
```

```
OK
```

Actions refused:

```sql
ALTER TABLE STAFF ADD COLUMN ID INTEGER;
ALTER TABLE STAFF ADD COLUMN HIRED DATE NOT NULL;
ALTER TABLE STAFF ALTER COLUMN NAME DROP DEFAULT;
ALTER TABLE NOPE ADD COLUMN X INTEGER;
ALTER TABLE STAFF DROP COLUMN DEPT;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.STAFF already has a column ID [ISO/IEC 9075:1992 §11.4 SR1] (at character 29)
turborel: SQLCODE -201 (SQLSTATE 23000): ANN.STAFF has rows, in which NOT NULL column HIRED would be null [ISO/IEC 9075:1992 §11.11 GR2]
turborel: SQLCODE -103 (SQLSTATE 2A000): column NAME of ANN.STAFF has no default to drop [ISO/IEC 9075:1992 §11.14 SR2] (at character 31)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.NOPE is not defined [ISO/IEC 9075:1992 §11.10 SR2] (at character 12)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected CASCADE or RESTRICT, found end of statement (at character 34)
```

## STANDARD

ISO/IEC 9075:1992 §11.10 to §11.17. A column dropped takes a table
constraint that reads it alone, even with RESTRICT [§11.15 SR4]. The
actions of an identity column and DROP EXPRESSION are ISO/IEC 9075-2:2023
§11.20 to §11.22, Features T178, F384, F385 and F386, and the drop of a
column a generation expression references is §11.23 SR7f and GR3. SET,
USING and RESEAL are extensions. SQL-86 has no ALTER TABLE.

## SEE ALSO

[CREATE TABLE](create-table.md), [DROP TABLE](drop-table.md),
[ALTER DOMAIN](alter-domain.md), [ALTER SEQUENCE](alter-sequence.md), [CREATE VIEW](create-view.md),
[Temporary Tables](../../user/09-temporary-tables.md)
