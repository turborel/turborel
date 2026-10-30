# Appendix A: SQLSTATE and SQLCODE

[Contents](README.md)

Every statement completes with an SQLSTATE, five characters, and an
SQLCODE, a number. The client shows both of an error or a warning:
`turborel: SQLCODE -103 (SQLSTATE 2A000): ...`. In a program,
[GET DIAGNOSTICS](../sql/statements/get-diagnostics.md) reads them.

## SQLSTATE values

An SQLSTATE is a class, its first two characters, and a subclass, the
other three; subclass 000 is the class alone. Class 00 is success, 01 a
warning, 02 no data; any other is an exception, the statement undone.
The classes of the standard are of ISO 9075; class TB is turborel's own.

| SQLSTATE | Condition |
|---|---|
| 00000 | Successful completion |
| 01001 | Warning: cursor operation conflict |
| 01002 | Warning: disconnect error |
| 01004 | Warning: string data, right truncation: a string cut to fit a parameter |
| 01006 | Warning: privilege not revoked |
| 01007 | Warning: privilege not granted |
| 01008 | Warning: implicit zero-bit padding: a bit string padded to fit a parameter |
| 01009 | Warning: search condition too long for the Information Schema |
| 0100A | Warning: query expression too long for the Information Schema |
| 02000 | No data |
| 07000 | Dynamic SQL error |
| 07001 | Dynamic SQL error: the arguments do not match the dynamic parameters |
| 07003 | Dynamic SQL error: a cursor specification cannot be executed |
| 07005 | Dynamic SQL error: a prepared statement is no cursor specification |
| 08000 | Connection exception |
| 08001 | Connection exception: the client cannot connect |
| 08002 | Connection exception: connection name in use |
| 08003 | Connection exception: connection does not exist |
| 08004 | Connection exception: the server refused the connection |
| 08006 | Connection exception: connection failure |
| 0A000 | Feature not supported: valid SQL turborel does not process |
| 0A001 | Feature not supported: multiple server transactions |
| 21000 | Cardinality violation: a subquery of one row gave more |
| 22000 | Data exception |
| 22001 | String data, right truncation: a string too long for its type |
| 22002 | Null value, no indicator parameter |
| 22003 | Numeric value out of range |
| 22005 | Error in assignment |
| 22007 | Invalid datetime format |
| 22008 | Datetime field overflow |
| 22009 | Invalid time zone displacement value |
| 22011 | Substring error |
| 22012 | Division by zero |
| 22015 | Interval field overflow |
| 22018 | Invalid character value for cast |
| 22019 | Invalid escape character |
| 22022 | Indicator overflow |
| 22025 | Invalid escape sequence |
| 22026 | String data, length mismatch: a bit string not of its BIT(n)'s length |
| 22027 | Trim error |
| 23000 | Integrity constraint violation |
| 24000 | Invalid cursor state |
| 25000 | Invalid transaction state |
| 26000 | Invalid SQL statement name |
| 27000 | Triggered data change violation: a row changed twice by referential actions |
| 28000 | Invalid authorization specification |
| 2A000 | Syntax error or access rule violation in a direct SQL statement |
| 2B000 | Dependent privilege descriptors still exist |
| 2E000 | Invalid connection name |
| 35000 | Invalid condition number |
| 37000 | Syntax error or access rule violation in a dynamic SQL statement |
| 3D000 | Invalid catalog name |
| 3F000 | Invalid schema name |
| 40000 | Transaction rollback |
| 40001 | Transaction rollback: serialization failure |
| 40002 | Transaction rollback: integrity constraint violation, at commit |
| 44000 | With check option violation |
| TB910 | The Rust API was misused, or a defect |
| TB911 | Conflict with another transaction: the statement undone, the transaction going on |
| TB912 | Refused by the transaction policy |
| TB914 | A table damaged by a change that could not be undone |
| TB915 | A file could not be read, written or synced |
| TB916 | Stored data failed its checksum or format |
| TB918 | The memory budget is spent, or a buffer pool has no frame free |
| TB920 | The statement was cancelled |

Statements are run directly, so an error of the rules of a statement's
form, its names or its privileges is class 2A; while preparing one, class
37.

## SQLCODE values

The SQLCODE is 0 of a success or a warning, 100 of no data, and
negative of an exception:

| SQLCODE | Exception |
|---|---|
| -101 | Lexical: a character or a token not of SQL |
| -102 | Format: the statement not written as its syntax says |
| -103 | A Syntax Rule broken: a name not defined, a type not fitting, a rule of the statement |
| -104 | A privilege not held |
| -201 | A constraint broken |
| -301 | A data exception |
| -501 | A cursor's state |
| -811 | Cardinality |
| -900 | Valid SQL turborel does not process |
| -910 | The Rust API misused, or a defect |
| -911 | A conflict with another transaction |
| -912 | The transaction policy |
| -913 | A commit failed, the transaction rolled back |
| -914 | A table damaged by a failed undo |
| -915 | An I/O error |
| -916 | Corrupt stored data |
| -917 | An invalid transaction state |
| -918 | The memory budget spent, or a buffer pool with every frame held |
| -919 | A connection exception |
| -920 | The statement cancelled |
| -921 | A dynamic SQL error |

The standard: ISO/IEC 9075:1992 §22.1, Table 23; X3.135-1986 §7.3, of
SQLCODE.
