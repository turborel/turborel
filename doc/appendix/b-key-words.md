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

SQL:2023 reserves BINARY, BOOLEAN, BTRIM, DECFLOAT, EVERY, GREATEST,
LEAST, LIKE_REGEX, LPAD, LTRIM, OCCURRENCES_REGEX, POSITION_REGEX, RPAD,
RTRIM, START, SUBSTRING_REGEX, TRANSLATE_REGEX and VARBINARY, which SQL-92 does not, so
a column of an older database may be named any of them. They stay identifiers here, key words only where
they are read. BINARY, BOOLEAN, DECFLOAT and VARBINARY are data types
where a data type is written, so a domain cannot be named BINARY,
BOOLEAN or VARBINARY, and one named DECFLOAT is never the type a column
of DECFLOAT is. BTRIM, EVERY, GREATEST, LEAST, LPAD, LTRIM, RPAD and
RTRIM are functions before a parenthesis, so a table's column of one of
those names is written in double quotes, `"LPAD"` say, only where a
parenthesis follows it. LIKE_REGEX is a predicate after a value, and
FLAG, which SQL:2023 does not reserve, is read only after its pattern, so
`LIKE_REGEX LIKE_REGEX 'a' FLAG FLAG` compares two columns of those names.
OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX and TRANSLATE_REGEX are
functions before a parenthesis, as the trims are. Within POSITION_REGEX,
START and AFTER are words unless IN or FLAG follows, so
`POSITION_REGEX(START IN S)` searches S for the value of a column START.
OCCURRENCE, CHARACTERS and OCTETS are read only where those functions
take them, and after USING in CHAR_LENGTH, POSITION and SUBSTRING, and
after a character string type's length. FORMAT, which SQL:2023 does not
reserve either, is read only after a CAST's target.

The standard: ISO/IEC 9075:1992 §5.2; ISO/IEC 9075-2:2023 §5.2.
