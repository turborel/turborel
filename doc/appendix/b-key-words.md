# Appendix B: Key Words

[Contents](README.md)

## Reserved words

The 226 reserved words of SQL-92, each a key word in any case,
and none an identifier unless delimited: a column named VALUE is written
`"VALUE"`. Some name statements turborel does not process; they are
reserved all the same.

**A** ABSOLUTE · ACTION · ADD · ALL · ALLOCATE · ALTER · AND · ANY · ARE · AS · ASC · ASSERTION · AT · AUTHORIZATION · AVG  
**B** BEGIN · BETWEEN · BIT · BIT_LENGTH · BOTH · BY  
**C** CASCADE · CASCADED · CASE · CAST · CATALOG · CHAR · CHARACTER · CHARACTER_LENGTH · CHAR_LENGTH · CHECK · CLOSE · COALESCE · COLLATE · COLLATION · COLUMN · COMMIT · CONNECT · CONNECTION · CONSTRAINT · CONSTRAINTS · CONTINUE · CONVERT · CORRESPONDING · COUNT · CREATE · CROSS · CURRENT · CURRENT_DATE · CURRENT_TIME · CURRENT_TIMESTAMP · CURRENT_USER · CURSOR  
**D** DATE · DAY · DEALLOCATE · DEC · DECIMAL · DECLARE · DEFAULT · DEFERRABLE · DEFERRED · DELETE · DESC · DESCRIBE · DESCRIPTOR · DIAGNOSTICS · DISCONNECT · DISTINCT · DOMAIN · DOUBLE · DROP  
**E** ELSE · END · ESCAPE · EXCEPT · EXCEPTION · EXEC · EXECUTE · EXISTS · EXTERNAL · EXTRACT  
**F** FALSE · FETCH · FIRST · FLOAT · FOR · FOREIGN · FOUND · FROM · FULL  
**G** GET · GLOBAL · GO · GOTO · GRANT · GROUP  
**H** HAVING · HOUR  
**I** IDENTITY · IMMEDIATE · IN · INDICATOR · INITIALLY · INNER · INPUT · INSENSITIVE · INSERT · INT · INTEGER · INTERSECT · INTERVAL · INTO · IS · ISOLATION  
**J** JOIN  
**K** KEY  
**L** LANGUAGE · LAST · LEADING · LEFT · LEVEL · LIKE · LOCAL · LOWER  
**M** MATCH · MAX · MIN · MINUTE · MODULE · MONTH  
**N** NAMES · NATIONAL · NATURAL · NCHAR · NEXT · NO · NOT · NULL · NULLIF · NUMERIC  
**O** OCTET_LENGTH · OF · ON · ONLY · OPEN · OPTION · OR · ORDER · OUTER · OUTPUT · OVERLAPS  
**P** PAD · PARTIAL · POSITION · PRECISION · PREPARE · PRESERVE · PRIMARY · PRIOR · PRIVILEGES · PROCEDURE · PUBLIC  
**R** READ · REAL · REFERENCES · RELATIVE · RESTRICT · REVOKE · RIGHT · ROLLBACK · ROWS  
**S** SCHEMA · SCROLL · SECOND · SECTION · SELECT · SESSION · SESSION_USER · SET · SIZE · SMALLINT · SOME · SPACE · SQL · SQLCODE · SQLERROR · SQLSTATE · SUBSTRING · SUM · SYSTEM_USER  
**T** TABLE · TEMPORARY · THEN · TIME · TIMESTAMP · TIMEZONE_HOUR · TIMEZONE_MINUTE · TO · TRAILING · TRANSACTION · TRANSLATE · TRANSLATION · TRIM · TRUE  
**U** UNION · UNIQUE · UNKNOWN · UPDATE · UPPER · USAGE · USER · USING  
**V** VALUE · VALUES · VARCHAR · VARYING · VIEW  
**W** WHEN · WHENEVER · WHERE · WITH · WORK · WRITE  
**Y** YEAR  
**Z** ZONE  

## Non-reserved words

SQL-92's non-reserved words are identifiers, and key words only where a
statement writes them: the isolation levels of SET TRANSACTION, the
items of GET DIAGNOSTICS.

ADA · C · CATALOG_NAME · CHARACTER_SET_CATALOG · CHARACTER_SET_NAME · CHARACTER_SET_SCHEMA · CLASS_ORIGIN · COBOL · COLLATION_CATALOG · COLLATION_NAME · COLLATION_SCHEMA · COLUMN_NAME · COMMAND_FUNCTION · COMMITTED · CONDITION_NUMBER · CONNECTION_NAME · CONSTRAINT_CATALOG · CONSTRAINT_NAME · CONSTRAINT_SCHEMA · CURSOR_NAME · DATA · DATETIME_INTERVAL_CODE · DATETIME_INTERVAL_PRECISION · DYNAMIC_FUNCTION · FORTRAN · LENGTH · MESSAGE_LENGTH · MESSAGE_OCTET_LENGTH · MESSAGE_TEXT · MORE · MUMPS · NAME · NULLABLE · NUMBER · PASCAL · PLI · REPEATABLE · RETURNED_LENGTH · RETURNED_OCTET_LENGTH · RETURNED_SQLSTATE · ROW_COUNT · SCALE · SCHEMA_NAME · SERIALIZABLE · SERVER_NAME · SUBCLASS_ORIGIN · TABLE_NAME · TYPE · UNCOMMITTED · UNNAMED

Of turborel's own extensions, INDEX, of CREATE, ALTER and DROP INDEX,
PASSWORD, of CREATE and ALTER USER, DATABASE, of SET DATABASE and
ALTER DATABASE, OWNER, of ALTER DATABASE, CANCEL, of CANCEL
CONNECTION, and RESEAL, of ALTER TABLE, are
identifiers too, key words only there; CANCEL only where CONNECTION
follows it, beginning a statement. DATABASE is that statement's
only when a setting's name follows it; in `SET DATABASE = 1` it is
the name of the setting SET sets.

SQL:2023 reserves ANY_VALUE, BEGIN_FRAME, BEGIN_PARTITION, BIGINT, BINARY, BOOLEAN, BTRIM, CUBE, CUME_DIST, CURRENT_ROW, CYCLE, DECFLOAT,
DENSE_RANK, END_FRAME, END_PARTITION, EVERY, FILTER, FIRST_VALUE, FRAME_ROW, GREATEST, GROUPING, GROUPS, LAG, LAST_VALUE, LEAD, LEAST, LIKE_REGEX,
LISTAGG, LPAD, LTRIM, NTH_VALUE, NTILE, OCCURRENCES_REGEX, OVER, PARTITION, PERCENT_RANK, POSITION_REGEX,
RANGE, RANK, ROLLUP, ROW_NUMBER, RPAD, RTRIM, START, SUBSTRING_REGEX, TRANSLATE_REGEX, VALUE_OF, VARBINARY, WIDTH_BUCKET, WINDOW
and WITHIN, which SQL-92 does not, so
a column of an older database may be named any of them. They stay identifiers here, key words only where
they are read. BIGINT, BINARY, BOOLEAN, DECFLOAT and VARBINARY are data types
where a data type is written, so a domain cannot be named BINARY,
BOOLEAN or VARBINARY, and one named BIGINT or DECFLOAT is never the type
a column of that type is. BTRIM, EVERY, GREATEST, LEAST, LPAD, LTRIM, RPAD,
RTRIM and WIDTH_BUCKET are functions before a parenthesis, so a table's column of one of
those names is written in double quotes, `"LPAD"` say, only where a
parenthesis follows it. LIKE_REGEX is a predicate after a value, and
FLAG, which SQL:2023 does not reserve, is read only after its pattern, so
`LIKE_REGEX LIKE_REGEX 'a' FLAG FLAG` compares two columns of those names.
OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX and TRANSLATE_REGEX are
functions before a parenthesis, as the trims are. Within POSITION_REGEX,
START and AFTER are words unless IN or FLAG follows, so
`POSITION_REGEX(START IN S)` searches S for the value of a column START.
PARTITION is read only where BY follows it after an operand of a join,
so `FROM T PARTITION` still names T PARTITION.
In a GROUP BY, ROLLUP and CUBE are read only before a parenthesis, and
GROUPING only before SETS, which SQL:2023 does not reserve. GROUPING is
the grouping operation before a parenthesis in a value, so `GROUP BY
ROLLUP, CUBE` groups by two columns of those names.
ANY_VALUE and LISTAGG are set functions only before a parenthesis. WITHIN
is read only before GROUP after LISTAGG's arguments, and OVERFLOW, which
SQL:2023 does not reserve, only after ON there, so `SELECT LISTAGG,
WITHIN FROM T` names two columns.

OVER is read only after a function's closing parenthesis, where a
parenthesis or a window's name follows it, so `SELECT COUNT(*) OVER FROM
T` names the count OVER. FILTER is read only after a set function's
closing parenthesis, where a parenthesis and WHERE follow it, so `SELECT
COUNT(*) FILTER FROM T` names the count FILTER. ROW_NUMBER, RANK, DENSE_RANK, PERCENT_RANK and
CUME_DIST are window functions only where `( ) OVER` follows, so a
function registered as RANK stays callable. NTILE, LEAD, LAG,
FIRST_VALUE, LAST_VALUE and NTH_VALUE are window functions only where,
after the closing parenthesis of their arguments, OVER follows, perhaps
after FROM FIRST or FROM LAST and RESPECT NULLS or IGNORE NULLS. RESPECT
and IGNORE, which neither SQL-92 nor SQL:2023 reserves, are read only
there. PARTITION is read in a
window only before BY too. WINDOW begins the WINDOW clause only where a
name and AS follow it, after the table expression's other clauses. RANGE
and GROUPS begin a frame only where ROWS, which SQL-92 reserves, may, and
PRECEDING, FOLLOWING, UNBOUNDED, EXCLUDE, TIES and OTHERS, which SQL:2023
does not reserve, are read only in a frame. A column may be named any of
them but ROWS. ROW_NUMBER is a nested window function only where one
row marker alone is between its parentheses, and VALUE_OF only where AT
and a row marker are among its arguments, outside any parentheses within
them. The six row markers are read only there, so each names a column
elsewhere and a function registered as VALUE_OF stays callable. Within
VALUE_OF's value, AT is a time zone's only before LOCAL or TIME.

SEQUENCE, INCREMENT, MAXVALUE, MINVALUE and RESTART, which SQL:2023 does
not reserve, and CYCLE and START are key words only in CREATE, ALTER and
DROP SEQUENCE, GRANT and REVOKE ON SEQUENCE, so a table may have columns of
those names. NEXT, which SQL-92 reserves, begins NEXT VALUE FOR.
OCCURRENCE, CHARACTERS and OCTETS are read only where those functions
take them, and after USING in CHAR_LENGTH, POSITION and SUBSTRING, and
after a character string type's length. FORMAT, which SQL:2023 does not
reserve either, is read only after a CAST's target.

GENERATED, ALWAYS, EXPRESSION and OVERRIDING, which SQL:2023 does not
reserve, and SYSTEM, which it does, are key words only in a column's
identity or generation, in ALTER COLUMN and in an INSERT's override
clause. GENERATED begins a column's identity or generation only where
ALWAYS or BY follows it, so a column may be of a domain named GENERATED.

TRUNCATE, which SQL:2023 reserves and SQL-92 does not, begins TRUNCATE
TABLE only where TABLE follows it at the start of a statement, so a table
or a column may be named TRUNCATE. RESTART is read after its table's name
too, before IDENTITY.

INCLUDING, EXCLUDING, DEFAULTS and DATA, which SQL:2023 does not reserve,
are key words only in a LIKE clause of CREATE TABLE and in its WITH DATA
and WITH NO DATA. GENERATED is read there too, after INCLUDING or
EXCLUDING.

NULLS, which SQL:2023 does not reserve either, is a key word only after
UNIQUE, in a table constraint and in the UNIQUE predicate, where NULLS
DISTINCT or NULLS NOT DISTINCT follows, and after a sort key, where NULLS
FIRST or NULLS LAST follows. A column may be named NULLS.

RECURSIVE, SEARCH and CYCLE, which SQL:2023 reserves and SQL-92 does not,
are key words only in a WITH. RECURSIVE is one after WITH unless AS or a
column list follows it, so `WITH RECURSIVE AS (...)` names a query
RECURSIVE. SEARCH and CYCLE are read after a query of the list, and
DEPTH and BREADTH after SEARCH. A column may be named any of them.

ARRAY, ROW, UNNEST, CARDINALITY, ARRAY_MAX_CARDINALITY, TRIM_ARRAY and
ARRAY_AGG, which SQL:2023 reserves and SQL-92 does not, are identifiers
too, key words only where they are read. ARRAY is one after a data type,
and before `[` or a parenthesis where a value is; ROW before a parenthesis
where a data type or a value is; UNNEST before a parenthesis where a table
is; and the four functions before a parenthesis, so a column of one of
those names is written in double quotes only there. ORDINALITY, which
SQL:2023 does not reserve, is read only after WITH, after UNNEST's
arrays. Of the descriptor items, DEGREE, CARDINALITY and
RETURNED_CARDINALITY are read only as items, TOP_LEVEL_COUNT only as a
header item, and NESTING and WITHOUT only after DESCRIBE's descriptor.

TYPE, FINAL, INSTANTIABLE, UNDER, SOURCE, REF, SPECIFIC, FUNCTION and
ROUTINE, which SQL-92 does not reserve, are key words only in CREATE TYPE,
DROP TYPE, GRANT and REVOKE, so a column may be named TYPE. A function a
distinct type's definition makes may be named by a key word, as one of a
NUMERIC source is, and is then invoked delimited, `"NUMERIC"(p)`. Of the
descriptor items, USER_DEFINED_TYPE_CATALOG, USER_DEFINED_TYPE_SCHEMA,
USER_DEFINED_TYPE_NAME and USER_DEFINED_TYPE_CODE are read only as items.

The standard: ISO/IEC 9075:1992 §5.2; ISO/IEC 9075-2:2023 §5.2.
