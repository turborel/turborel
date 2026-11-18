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
| 01003 | Warning: null value eliminated in set function: a set function left out a null |
| 01004 | Warning: string data, right truncation: a string cut to fit a parameter, or by CAST |
| 01005 | Warning: insufficient item descriptor areas: DESCRIBE of more than an SQL descriptor area's occurrences |
| 01006 | Warning: privilege not revoked |
| 01007 | Warning: privilege not granted |
| 01009 | Warning: search condition too long for the Information Schema |
| 0100A | Warning: query expression too long for the Information Schema |
| 01010 | Warning: column cannot be mapped: XMLMAP left out a column the user may read, whose type does not map to XML |
| 02000 | No data |
| 07000 | Dynamic SQL error |
| 07001 | Dynamic SQL error: the arguments do not match the dynamic parameters |
| 07002 | Dynamic SQL error: the INTO clause does not match the columns |
| 07003 | Dynamic SQL error: a cursor specification cannot be executed |
| 07004 | Dynamic SQL error: a USING clause is needed for the dynamic parameters |
| 07005 | Dynamic SQL error: a prepared statement is no cursor specification |
| 07006 | Dynamic SQL error: restricted data type attribute violation: a value of a type not cast to its parameter's or item's |
| 07007 | Dynamic SQL error: an INTO clause is needed for the columns |
| 07008 | Dynamic SQL error: invalid descriptor count: COUNT less than 0 or more than the occurrences |
| 07009 | Dynamic SQL error: invalid descriptor index: an item number not of the occurrences |
| 0700C | Dynamic SQL error: undefined DATA value: DATA got of an item of a row, or of an array's or a multiset's element |
| 0700D | Dynamic SQL error: invalid DATA target: DATA set of an item of a row, or of an array's or a multiset's element |
| 0700E | Dynamic SQL error: invalid LEVEL value: a LEVEL set out of order with the items before it |
| 08000 | Connection exception |
| 08001 | Connection exception: the client cannot connect |
| 08002 | Connection exception: connection name in use |
| 08003 | Connection exception: connection does not exist |
| 08004 | Connection exception: the server refused the connection |
| 08006 | Connection exception: connection failure |
| 08007 | Connection exception: transaction resolution unknown: a commit in doubt, its commit record not synced; decided when the database is opened again |
| 09000 | Triggered action exception: a trigger's action failed; the message gives the trigger and the SQLSTATE and message of its failure |
| 0A000 | Feature not supported: valid SQL Turborel does not process |
| 0A001 | Feature not supported: multiple server transactions |
| 0F001 | Locator exception: invalid specification: a large object locator freed, of a transaction that ended, or never given |
| 0N002 | SQL/XML mapping error: invalid XML character: XMLCAST, XMLELEMENT, XMLFOREST, XMLPI or XMLTEXT of a string holding a character no XML 1.1 character |
| 0W000 | Prohibited statement encountered during trigger execution: a savepoint statement in a procedure a trigger's action calls; told in the message of the 09000 it raises |
| 10000 | XQuery error: XMLDOCUMENT of a value holding an attribute standing alone; XMLCAST of text that is no value of the XML Schema type cast to, or of more than one atomic value; XMLELEMENT of an attribute after other content, or of two attributes of one name; any static, type or dynamic error of a query XMLQUERY, XMLEXISTS or XMLTABLE evaluates, or static error of the library module CREATE XQUERY MODULE registers, its message naming the error's code, as err:XPST0008 |
| 21000 | Cardinality violation: a subquery of one row gave more, or ELEMENT a multiset of more than one element |
| 22000 | Data exception: a path's `%` of an approximate number or one with a fraction; vectors of other dimension counts or formats |
| 22001 | String data, right truncation: a string too long for its type, JSON_QUERY's JSON text longer than its returned type whatever ON ERROR says among them; FROM_VECTOR of a vector whose text is longer than a character string |
| 22002 | Null value, no indicator parameter |
| 22003 | Numeric value out of range, a number of JSON text of more than 34 significant digits among them, wherever JSON text is read, IS JSON too; a vector element out of its format's range, NaN or infinite, or a vector distance that overflows |
| 22004 | Null value not allowed: a null name of a member of JSON_OBJECT or JSON_OBJECTAGG |
| 22005 | Error in assignment |
| 22007 | Invalid datetime format |
| 22008 | Datetime field overflow |
| 2200D | Invalid escape octet: an octet LIKE's escape octet not of one octet |
| 2200E | Null value in array target: an element assigned of a null array |
| 2200H | Sequence generator limit exceeded: NEXT VALUE FOR of a generator of NO CYCLE past its last value |
| 2200L | Not an XML document: a value stored in, or cast to, an XML(DOCUMENT) type that is no document, or XMLSERIALIZE DOCUMENT of one; or the document of CREATE XML SCHEMA the null value or an XML value that is no document |
| 2200M | Invalid XML document: XMLPARSE DOCUMENT of text that is no well-formed document, or CREATE XML SCHEMA of one; XMLVALIDATE DOCUMENT of a value that is no document |
| 2200N | Invalid XML content: XMLPARSE CONTENT of text that is no well-formed content, or content XMLELEMENT or XMLFOREST maps to XML that is none; XMLVALIDATE CONTENT of a value that is not one document node |
| 2200Q | Multiset value overflow: a multiset of more than 65,535 elements |
| 2200R | XML value overflow: XMLAGG of values holding more than 64 MiB together |
| 2200S | Invalid comment: XMLCOMMENT of text holding two hyphens together, ending with one, or holding a character no XML 1.1 character |
| 2200T | Invalid processing instruction: XMLPI of text holding '?>' |
| 2200U | Not an XQuery document node: a value stored in, or cast to, an XML(CONTENT) type that is not one document node |
| 2200V | Invalid XQuery context item: XMLQUERY, XMLEXISTS or XMLTABLE passed a context item of more than one item |
| 2200W | XQuery serialization error: an XML value with no serialization, as an attribute standing alone, serialized or sent to a client |
| 22009 | Invalid time zone displacement value |
| 22011 | Substring error |
| 22012 | Division by zero, of a vector among them |
| 22013 | Invalid preceding or following size in window function: a frame's offset negative or null |
| 22014 | Invalid argument for NTILE function: a number of tiles below 1 |
| 22015 | Interval field overflow |
| 22016 | Invalid argument for NTH_VALUE function: a row below 1 |
| 22018 | Invalid character value for cast, text that is no vector among them |
| 22019 | Invalid escape character |
| 2201E | Invalid argument for natural logarithm: LN of 0 or below |
| 2201F | Invalid argument for power function: POWER of 0 to a negative power, or of a negative number to a power not whole; SQRT of a negative number |
| 2201J | XQuery sequence cannot be validated: XMLVALIDATE SEQUENCE of an atomic value, an attribute or a text node |
| 2201K | XQuery document node cannot be validated: XMLVALIDATE of a document node with a text child |
| 2201L | No XML schema found: XMLVALIDATE of no ACCORDING TO of an element whose namespace has no registered XML Schema the user may use |
| 2201M | Element namespace not declared: an element clause names a namespace its registered XML Schema has not |
| 2201N | Global element not declared: an element clause names an element its registered XML Schema does not declare |
| 2201P | No XML element with the specified QName: XMLVALIDATE of an element not of the name its element clause gives |
| 2201Q | No XML element with the specified namespace: XMLVALIDATE of an element not of the namespace its element clause gives |
| 2201R | Validation failure: XMLVALIDATE, or CAST BY VALUE to a type of XMLSCHEMA, of an element not valid; a value no validation made stored in, or cast to, a type of XMLSCHEMA |
| 2201S | Invalid XQuery regular expression: a pattern no XQuery regular expression |
| 2201T | Invalid XQuery option flag: a flag of a character none of s, m, i, x and q |
| 2201U | Attempt to replace a zero-length string: TRANSLATE_REGEX of a match of no characters, or of a pattern that matches the empty string |
| 2201V | Invalid XQuery replacement string: a `$` before no digit, or a `\` before neither `$` nor `\` |
| 2201W | Invalid row count in fetch first clause: a count of FETCH FIRST below 1, or null, or a percentage of the rows whose count is |
| 2201X | Invalid row count in result offset clause: an offset below 0, or null |
| 22022 | Indicator overflow |
| 22025 | Invalid escape sequence |
| 22027 | Trim error |
| 2202E | Array element error: an element referenced not of the array, or an array trimmed of more elements than it has; VECTOR_ELEMENT or VECTOR_SLICE outside the vector, or VECTOR_TO_ARRAY of more elements than an array holds |
| 2202F | Array data, right truncation: an array, or a multiset cast to an array, of more elements than its target's maximum cardinality |
| 2202J | Invalid argument for row pattern navigation function: a logical or physical offset of PREV, NEXT, FIRST or LAST below 0, or null |
| 2202K | Skip to non-existent row: AFTER MATCH SKIP TO a variable no row of the match is mapped to |
| 2202L | Skip to first row of match: AFTER MATCH SKIP TO a variable whose row it names is the match's first |
| 2202V | Non-Boolean SQL/JSON item: a path's boolean() of an item neither a truth value nor a string |
| 2202W | Non-date SQL/JSON item: a path's date() of an item that is no string |
| 2202X | Non-string SQL/JSON item: a path's string() of an array, an object or the SQL/JSON null |
| 2202Y | Non-time SQL/JSON item: a path's time() of an item that is no string, or time_tz() of one neither a string nor a number |
| 2202Z | Non-timestamp SQL/JSON item: a path's timestamp() of an item that is no string, or timestamp_tz() of one neither a string nor a number |
| 22030 | Duplicate JSON object key value: JSON_OBJECT or JSON_OBJECTAGG WITH UNIQUE KEYS of two members of one name |
| 22031 | Invalid argument for SQL/JSON datetime function: a path's datetime() of an item that is no character string, or of a string that is no date, time or timestamp, or that its template does not read |
| 22032 | Invalid JSON text: a context item or PASSING argument of JSON_EXISTS, JSON_VALUE or JSON_QUERY, or a value FORMAT JSON of a constructor, that is no JSON text, or an item JSON_QUERY serializes that has none, as a datetime, or a parent's item of no JSON text chained to JSON_TABLE's NESTED COLUMNS under ERROR ON ERROR |
| 22033 | Invalid SQL/JSON subscript: an array subscript of a path that is not one number, or in strict mode outside the array |
| 22034 | More than one SQL/JSON item: JSON_VALUE, or JSON_QUERY without a wrapper, of a path that gives two items or more |
| 22035 | No SQL/JSON item: JSON_VALUE or JSON_QUERY of ERROR ON EMPTY, of a path that gives no item |
| 22036 | Non-numeric SQL/JSON item: a path's double(), bigint(), integer(), decimal() or number() of an item neither a number nor a string, or ceiling(), floor() or abs() of one no number |
| 22037 | Non-unique keys in a JSON object: JSON text WITH UNIQUE KEYS of an object of a key twice, as a value FORMAT JSON of JSON_OBJECT WITH UNIQUE KEYS |
| 22038 | Singleton SQL/JSON item required: an operand of a path's arithmetic that is not one number |
| 22039 | SQL/JSON array not found: an array accessor, or in strict mode size(), of an item that is no array |
| 2203A | SQL/JSON member not found: in strict mode, a member accessor of an item that is no object with the member |
| 2203B | SQL/JSON number not found: a path's sign of an item that is no number |
| 2203C | SQL/JSON object not found: in strict mode, `.*` of an item that is no object |
| 2203D | Too many JSON array elements: JSON_ARRAY or JSON_ARRAYAGG of more than 1,048,576 elements |
| 2203E | Too many JSON object members: JSON_OBJECT or JSON_OBJECTAGG of more than 1,048,576 members |
| 2203F | SQL/JSON scalar required: JSON_VALUE of an array or an object |
| 2203G | SQL/JSON item cannot be cast to target type: JSON_VALUE of an item whose type does not cast to the returned type, as true to INTEGER |
| 23000 | Integrity constraint violation |
| 24000 | Invalid cursor state |
| 25000 | Invalid transaction state |
| 25006 | Invalid transaction state: read-only SQL-transaction, of TRUNCATE TABLE |
| 26000 | Invalid SQL statement name |
| 27000 | Triggered data change violation: a row changed twice by referential actions |
| 28000 | Invalid authorization specification |
| 2A000 | Syntax error or access rule violation in a direct SQL statement |
| 2B000 | Dependent privilege descriptors still exist |
| 2E000 | Invalid connection name |
| 2F005 | SQL routine exception: function executed no return statement, of a function whose body is no RETURN |
| 2FF01 | SQL/MM Full-Text exception: invalid search expression, a pattern not of the [pattern language](../sql/09-fulltext-patterns.md) |
| 2FF02 | SQL/MM Full-Text exception: invalid language specification, a language null, empty or unknown |
| 2FF03 | SQL/MM Full-Text exception: effectively empty search expression, every word of the pattern a stop word |
| 2H000 | Invalid collation name |
| 33000 | Invalid SQL descriptor name |
| 34000 | Invalid cursor name: of a cursor allocated, or a positioned statement prepared |
| 35000 | Invalid condition number |
| 36002 | Cursor sensitivity exception: request failed, of TRUNCATE TABLE while a cursor of the transaction reads the table |
| 37000 | Syntax error or access rule violation in a dynamic SQL statement |
| 3B000 | Savepoint exception: the class of 3B001 and 3B002 |
| 3B001 | Savepoint exception: invalid specification: RELEASE or ROLLBACK TO a savepoint not of the current savepoint level, or established before the atomic execution context running began |
| 3B002 | Savepoint exception: too many: a transaction's 1,025th savepoint |
| 3C000 | Ambiguous cursor name: a cursor declared and one allocated, of one name |
| 3D000 | Invalid catalog name: an identifier chain of four or more that names no catalog, column or field |
| 3F000 | Invalid schema name |
| 40000 | Transaction rollback |
| 40001 | Transaction rollback: serialization failure |
| 40002 | Transaction rollback: integrity constraint violation, at commit |
| 44000 | With check option violation |
| TB910 | The Rust API was misused, or a defect, or a registered vector distance function that breaks its contract |
| TB911 | Conflict with another transaction: the statement undone, the transaction going on |
| TB912 | Refused by the transaction policy |
| TB914 | A table damaged by a change that could not be undone |
| TB915 | A file could not be read, written or synced |
| TB916 | Stored data failed its checksum or format |
| TB918 | The memory budget is spent, a buffer pool has no frame free, or a recursive query takes more iterations than EXECUTOR.MAX_ITERATIONS |
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
| -900 | Valid SQL Turborel does not process |
| -910 | The Rust API misused, or a defect |
| -911 | A conflict with another transaction |
| -912 | The transaction policy |
| -913 | A commit failed, the transaction rolled back |
| -914 | A table damaged by a failed undo |
| -915 | An I/O error |
| -916 | Corrupt stored data |
| -917 | An invalid transaction state |
| -918 | The memory budget spent, a buffer pool with every frame held, or a recursive query past its iterations |
| -919 | A connection exception |
| -920 | The statement cancelled |
| -921 | A dynamic SQL error |
| -922 | A commit in doubt: whether it committed is known once the database is opened again |

The standard: ISO/IEC 9075:1992 §22.1, Table 23; X3.135-1986 §7.3, of
SQLCODE.
