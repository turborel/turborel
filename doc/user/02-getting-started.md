# Getting Started

[Contents](README.md)

## Connecting with the turborel client

With a server running ([Running the Server](../operator/02-turboreld.md)),
connect to it with `turborel`, as a user of the database:

```
$ turborel -U ann -W
password:
turborel 0.0.0 (alpha), the command-line client
Copyright 2026, Turborel contributors.
Distributed under the BSD 2-Clause License.

Connected to turborel 0.0.0 (alpha) at 127.0.0.1:7422, as ANN.
End each statement with a semicolon; end of input (Ctrl-D) quits.

turborel>
```

`-W` asks for the password. Each statement runs once a semicolon ends
it; it may go over many lines.

## A first schema

A user's tables are kept in a schema, named as the user:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE DEPARTMENTS (CODE CHAR(3) PRIMARY KEY, NAME VARCHAR(30) NOT NULL);
CREATE TABLE EMPLOYEES (
    ID     INTEGER PRIMARY KEY,
    NAME   VARCHAR(30) NOT NULL,
    DEPT   CHAR(3) REFERENCES DEPARTMENTS,
    SALARY DECIMAL(9,2) CHECK (SALARY > 0));
INSERT INTO DEPARTMENTS VALUES ('ENG', 'Engineering'), ('OPS', 'Operations');
INSERT INTO EMPLOYEES VALUES (1, 'Ada', 'ENG', 7200), (2, 'Grace', 'ENG', 8100), (3, 'Edsger', 'OPS', 5400);
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
```

The tables say what they hold, and what they refuse: each employee has
an ID of its own, a name, a department that exists, and a salary above
0. COMMIT makes the changes lasting.

## A first query

```sql
SELECT NAME, SALARY FROM EMPLOYEES WHERE DEPT = 'ENG' ORDER BY SALARY DESC;
SELECT D.NAME, COUNT(*) AS STAFF, AVG(E.SALARY) AS AVERAGE
  FROM DEPARTMENTS D JOIN EMPLOYEES E ON E.DEPT = D.CODE
 GROUP BY D.NAME ORDER BY D.NAME;
```

```
 NAME                           |      SALARY
--------------------------------+-------------
 Grace                          |     8100.00
 Ada                            |     7200.00
(2 rows)
 NAME                           |                STAFF |              AVERAGE
--------------------------------+----------------------+----------------------
 Engineering                    |                    2 |            7650.0000
 Operations                     |                    1 |            5400.0000
(2 rows)
```

And what the tables refuse:

```sql
INSERT INTO EMPLOYEES VALUES (4, 'Barbara', 'XYZ', 6000);
```

```
turborel: SQLCODE -201 (SQLSTATE 23000): a row of ANN.EMPLOYEES references no row of ANN.DEPARTMENTS: FOREIGN KEY ANN.EMPLOYEES_FOREIGN1 [ISO/IEC 9075:1992 §11.8 GR2]
```

There is no department XYZ. An error gives its SQLCODE and SQLSTATE,
what is wrong, and the rule of the standard that says so
([Errors and Diagnostics](10-errors.md)).

## Ending a session

The end of input, Ctrl-D, ends the session. A transaction left open is
committed as the session ends, unless the setting SESSION_END is
ROLLBACK; end it with COMMIT or ROLLBACK to be sure. A commit that fails
as the session ends is told, as any statement's failure is.
