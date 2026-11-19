//! \[EXTENSION\] SQLSTATE values beside SQLCODE values: a class of two
//! characters and a subclass of three [ISO/IEC 9075:1992 §22.1, Table 23].
//! X3.135-1986 has only SQLCODE; SQLSTATE is its successor's preferred
//! status parameter.
//!
//! Statements are invoked directly (README, conformance), so a statement
//! that breaks a Format, Syntax Rule or Access Rule raises the class of
//! direct invocation, 2A [ISO/IEC 9075:1992 §20.1 GR5]. Conditions this
//! implementation defines are in class TB, as a class beginning with T is
//! implementation-defined [§22.1]: the subclass is the SQLCODE's digits.

use std::fmt;

use super::Code;

/// \[EXTENSION\] An SQLSTATE value [ISO/IEC 9075:1992 §22.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlState([u8; 5]);

impl SqlState {
    /// The value of five characters `bytes`, each a digit or a simple Latin
    /// upper case letter, for the tables of other modules (`error/spatial.rs`).
    pub(crate) const fn of_bytes(bytes: [u8; 5]) -> SqlState {
        SqlState(bytes)
    }

    /// Successful completion.
    pub const SUCCESSFUL_COMPLETION: SqlState = SqlState(*b"00000");
    /// No data.
    pub const NO_DATA: SqlState = SqlState(*b"02000");
    /// Warning: cursor operation conflict [ISO/IEC 9075:1992 §11.8 GR8].
    pub const CURSOR_OPERATION_CONFLICT: SqlState = SqlState(*b"01001");
    /// Warning: privilege not revoked [ISO/IEC 9075:1992 §11.37 GR8, GR9].
    pub const PRIVILEGE_NOT_REVOKED: SqlState = SqlState(*b"01006");
    /// Warning: privilege not granted [ISO/IEC 9075:1992 §11.36 GR14, GR15].
    pub const PRIVILEGE_NOT_GRANTED: SqlState = SqlState(*b"01007");
    /// Warning: search condition too long for information schema [ISO/IEC
    /// 9075:1992 §11.9 GR2, §11.34 GR4].
    pub const SEARCH_CONDITION_TOO_LONG: SqlState = SqlState(*b"01009");
    /// Warning: query expression too long for information schema [ISO/IEC
    /// 9075:1992 §11.19 GR13].
    pub const QUERY_EXPRESSION_TOO_LONG: SqlState = SqlState(*b"0100A");
    /// Warning: statement too long for information schema [ISO/IEC
    /// 9075-2:2023 §11.49 GR4].
    pub const STATEMENT_TOO_LONG: SqlState = SqlState(*b"0100F");
    /// Warning: string data, right truncation [ISO/IEC 9075:1992 §9.1
    /// GR3b, GR3e; §6.10 GR5c ii, GR6c ii; ISO/IEC 9075-2:2023 §9.1 GR7i,
    /// GR7l; §6.13 GR13].
    pub const STRING_TRUNCATED: SqlState = SqlState(*b"01004");
    /// Warning: null value eliminated in set function [ISO/IEC 9075:1992
    /// §6.5 GR1b].
    pub const NULL_VALUE_ELIMINATED: SqlState = SqlState(*b"01003");
    /// Warning: column cannot be mapped [ISO/IEC 9075-14:2006 draft §9.11
    /// GR3, §9.14 GR3, §9.17 GR3, §23.1].
    pub const COLUMN_CANNOT_BE_MAPPED: SqlState = SqlState(*b"01010");
    /// Dependent privilege descriptors still exist [ISO/IEC 9075:1992
    /// §11.37 GR2b].
    pub const DEPENDENT_PRIVILEGES: SqlState = SqlState(*b"2B000");
    /// Feature not supported.
    pub const FEATURE_NOT_SUPPORTED: SqlState = SqlState(*b"0A000");
    /// Cardinality violation.
    pub const CARDINALITY_VIOLATION: SqlState = SqlState(*b"21000");
    /// Connection exception [ISO/IEC 9075:1992 §22.1, Table 23].
    pub const CONNECTION_EXCEPTION: SqlState = SqlState(*b"08000");
    /// Connection exception: SQL-server rejected establishment of
    /// SQL-connection.
    pub const CONNECTION_REJECTED: SqlState = SqlState(*b"08004");
    /// Connection exception: SQL-client unable to establish SQL-connection.
    pub const UNABLE_TO_CONNECT: SqlState = SqlState(*b"08001");
    /// Connection exception: connection name in use.
    pub const CONNECTION_NAME_IN_USE: SqlState = SqlState(*b"08002");
    /// Connection exception: connection does not exist.
    pub const CONNECTION_DOES_NOT_EXIST: SqlState = SqlState(*b"08003");
    /// Feature not supported: multiple server transactions.
    pub const MULTIPLE_SERVER_TRANSACTIONS: SqlState = SqlState(*b"0A001");
    /// Invalid connection name.
    pub const INVALID_CONNECTION_NAME: SqlState = SqlState(*b"2E000");
    /// [ISO/IEC 9075-2:2023 §19.10 GR1] Invalid collation name.
    pub const INVALID_COLLATION_NAME: SqlState = SqlState(*b"2H000");
    /// Warning: disconnect error.
    pub const DISCONNECT_ERROR: SqlState = SqlState(*b"01002");
    /// Connection exception: connection failure.
    pub const CONNECTION_FAILURE: SqlState = SqlState(*b"08006");
    /// Invalid authorization specification.
    pub const INVALID_AUTHORIZATION: SqlState = SqlState(*b"28000");
    /// [ISO/IEC 9075-2:2023 §12.5 GR1, §19.3 GR3, GR4] Invalid role
    /// specification.
    pub const INVALID_ROLE_SPECIFICATION: SqlState = SqlState(*b"0P000");
    /// Dynamic SQL error [ISO/IEC 9075:1992 §22.1, Table 23].
    pub const DYNAMIC_SQL_ERROR: SqlState = SqlState(*b"07000");
    /// Dynamic SQL error: using clause does not match dynamic parameter
    /// specifications.
    pub const USING_CLAUSE_MISMATCH: SqlState = SqlState(*b"07001");
    /// Dynamic SQL error: using clause does not match target
    /// specifications.
    pub const USING_CLAUSE_TARGETS_MISMATCH: SqlState = SqlState(*b"07002");
    /// Dynamic SQL error: cursor specification cannot be executed.
    pub const CURSOR_SPECIFICATION_NOT_EXECUTED: SqlState = SqlState(*b"07003");
    /// Dynamic SQL error: using clause required for dynamic parameters.
    pub const USING_CLAUSE_REQUIRED_PARAMETERS: SqlState = SqlState(*b"07004");
    /// Dynamic SQL error: prepared statement not a cursor specification.
    pub const NOT_A_CURSOR_SPECIFICATION: SqlState = SqlState(*b"07005");
    /// Dynamic SQL error: restricted data type attribute violation.
    pub const RESTRICTED_DATA_TYPE: SqlState = SqlState(*b"07006");
    /// Dynamic SQL error: using clause required for result fields.
    pub const USING_CLAUSE_REQUIRED_RESULTS: SqlState = SqlState(*b"07007");
    /// Dynamic SQL error: invalid descriptor count.
    pub const INVALID_DESCRIPTOR_COUNT: SqlState = SqlState(*b"07008");
    /// Dynamic SQL error: invalid descriptor index.
    pub const INVALID_DESCRIPTOR_INDEX: SqlState = SqlState(*b"07009");
    /// [ISO/IEC 9075-2:2023 §20.4 GR5a, GR5b] Dynamic SQL error: undefined
    /// DATA value.
    pub const UNDEFINED_DATA_VALUE: SqlState = SqlState(*b"0700C");
    /// [ISO/IEC 9075-2:2023 §20.5 GR4a] Dynamic SQL error: invalid DATA
    /// target.
    pub const INVALID_DATA_TARGET: SqlState = SqlState(*b"0700D");
    /// [ISO/IEC 9075-2:2023 §20.5 GR4b] Dynamic SQL error: invalid LEVEL
    /// value.
    pub const INVALID_LEVEL_VALUE: SqlState = SqlState(*b"0700E");
    /// [ISO/IEC 9075-2:2023 §20.28 GR11b ii] Dynamic SQL error: invalid
    /// pass-through surrogate value.
    pub const INVALID_SURROGATE: SqlState = SqlState(*b"0700G");
    /// [ISO/IEC 9075-2:2023 §20.28 GR1] Dynamic SQL error: PIPE ROW not
    /// during PTF execution.
    pub const PIPE_ROW_OUTSIDE: SqlState = SqlState(*b"0700H");
    /// Warning: insufficient item descriptor areas.
    pub const INSUFFICIENT_ITEM_DESCRIPTOR_AREAS: SqlState = SqlState(*b"01005");
    /// Invalid SQL statement name.
    pub const INVALID_STATEMENT_NAME: SqlState = SqlState(*b"26000");
    /// Invalid SQL descriptor name.
    pub const INVALID_DESCRIPTOR_NAME: SqlState = SqlState(*b"33000");
    /// Invalid cursor name.
    pub const INVALID_CURSOR_NAME: SqlState = SqlState(*b"34000");
    /// Ambiguous cursor name.
    pub const AMBIGUOUS_CURSOR_NAME: SqlState = SqlState(*b"3C000");
    /// Syntax error or access rule violation in dynamic SQL statement.
    pub const DYNAMIC_SYNTAX_ERROR: SqlState = SqlState(*b"37000");
    /// [ISO/IEC 9075-4:2006 draft §14.6 GR2b, §20.1] Case not found for
    /// case statement.
    pub const CASE_NOT_FOUND: SqlState = SqlState(*b"20000");
    /// [ISO/IEC 9075-4:2006 draft §17.1 GR, §20.1] Diagnostics exception:
    /// stacked diagnostics accessed without active handler.
    pub const STACKED_WITHOUT_HANDLER: SqlState = SqlState(*b"0Z002");
    /// [ISO/IEC 9075-4:2006 draft §17.3 GR, §20.1] Resignal when handler
    /// not active.
    pub const RESIGNAL_NOT_ACTIVE: SqlState = SqlState(*b"0K000");
    /// [ISO/IEC 9075-4:2006 draft §17.2 GR, §20.1] Unhandled user-defined
    /// exception.
    pub const UNHANDLED_USER_EXCEPTION: SqlState = SqlState(*b"45000");
    /// Data exception.
    pub const DATA_EXCEPTION: SqlState = SqlState(*b"22000");
    /// Data exception: invalid row version [ISO/IEC 9075-2:2023 §9.17
    /// GR16a iii 1, §15.14 GR10a iii 1 A]: a row changed whose system-time
    /// start is later than the transaction timestamp.
    pub const INVALID_ROW_VERSION: SqlState = SqlState(*b"2201H");
    /// Data exception: string data, right truncation.
    pub const STRING_DATA_RIGHT_TRUNCATION: SqlState = SqlState(*b"22001");
    /// [ISO/IEC 9075-2:2023 §4.2.1, §24.1] Data exception: character not
    /// in repertoire.
    pub const CHARACTER_NOT_IN_REPERTOIRE: SqlState = SqlState(*b"22021");
    /// [ISO/IEC 9075-2:2023 §14.17 GR1a, §14.18 GR1a, §24.1] Locator
    /// exception: invalid specification.
    pub const LOCATOR_INVALID_SPECIFICATION: SqlState = SqlState(*b"0F001");
    /// Data exception: null value, no indicator parameter.
    pub const NULL_VALUE_NO_INDICATOR: SqlState = SqlState(*b"22002");
    /// Data exception: numeric value out of range.
    pub const NUMERIC_VALUE_OUT_OF_RANGE: SqlState = SqlState(*b"22003");
    /// Data exception: error in assignment.
    pub const ERROR_IN_ASSIGNMENT: SqlState = SqlState(*b"22005");
    /// Data exception: division by zero.
    pub const DIVISION_BY_ZERO: SqlState = SqlState(*b"22012");
    /// [ISO/IEC 9075-2:2023 §24.1, Table 38] Data exception, invalid
    /// period value: a period whose start is not before its end [§8.20
    /// GR2b].
    pub const INVALID_PERIOD_VALUE: SqlState = SqlState(*b"22020");
    /// [ISO/IEC 9075-2:2023 §24.1, Table 38] Data exception, most specific
    /// type mismatch.
    pub const MOST_SPECIFIC_TYPE_MISMATCH: SqlState = SqlState(*b"2200G");
    /// [ISO/IEC 9075-2:2023 §24.1, Table 38] Data exception, null value
    /// substituted for mutator subject parameter.
    pub const NULL_MUTATOR_SUBJECT: SqlState = SqlState(*b"2202D");
    /// [ISO/IEC 9075-2:2023 §24.1, Table 38] Invalid target type
    /// specification.
    pub const INVALID_TARGET_TYPE: SqlState = SqlState(*b"0D000");
    /// [ISO/IEC 9075-2:2023 §24.1, Table 38] Invalid transform group name
    /// specification.
    pub const INVALID_TRANSFORM_GROUP_NAME: SqlState = SqlState(*b"0S000");
    /// Data exception: invalid preceding or following size in window
    /// function [ISO/IEC 9075-2:2023 §7.15 GR5b].
    pub const INVALID_PRECEDING_OR_FOLLOWING_SIZE: SqlState = SqlState(*b"22013");
    /// Data exception: invalid argument for NTILE function [ISO/IEC
    /// 9075-2:2023 §6.10 GR1a ii 2].
    pub const INVALID_ARGUMENT_FOR_NTILE: SqlState = SqlState(*b"22014");
    /// Data exception: invalid argument for NTH_VALUE function [ISO/IEC
    /// 9075-2:2023 §6.10 GR1d ii 2].
    pub const INVALID_ARGUMENT_FOR_NTH_VALUE: SqlState = SqlState(*b"22016");
    /// Data exception: invalid argument for natural logarithm [ISO/IEC
    /// 9075-2:2023 §6.31 GR12b].
    pub const INVALID_ARGUMENT_FOR_NATURAL_LOGARITHM: SqlState = SqlState(*b"2201E");
    /// Data exception: invalid argument for power function [ISO/IEC
    /// 9075-2:2023 §6.31 GR14b, GR14e].
    pub const INVALID_ARGUMENT_FOR_POWER_FUNCTION: SqlState = SqlState(*b"2201F");
    /// Data exception: invalid argument for width bucket function [ISO/IEC
    /// 9075-2:2023 §6.31 GR17b, GR17c].
    pub const INVALID_ARGUMENT_FOR_WIDTH_BUCKET: SqlState = SqlState(*b"2201G");
    /// Data exception: invalid row count in fetch first clause [ISO/IEC
    /// 9075-2:2023 §7.17 GR7c].
    pub const INVALID_ROW_COUNT_IN_FETCH_FIRST: SqlState = SqlState(*b"2201W");
    /// Data exception: invalid row count in result offset clause [ISO/IEC
    /// 9075-2:2023 §7.17 GR6b].
    pub const INVALID_ROW_COUNT_IN_RESULT_OFFSET: SqlState = SqlState(*b"2201X");
    /// Data exception: invalid argument for row pattern navigation
    /// function [ISO/IEC 9075-2:2023 §6.27 GR2].
    pub const INVALID_ARGUMENT_FOR_ROW_PATTERN_NAVIGATION: SqlState = SqlState(*b"2202J");
    /// Data exception: skip to non-existent row [ISO/IEC 9075-2:2023 §7.7
    /// GR8].
    pub const SKIP_TO_NON_EXISTENT_ROW: SqlState = SqlState(*b"2202K");
    /// Data exception: skip to first row of match [ISO/IEC 9075-2:2023
    /// §7.7 GR8].
    pub const SKIP_TO_FIRST_ROW_OF_MATCH: SqlState = SqlState(*b"2202L");
    /// Data exception: invalid escape character.
    pub const INVALID_ESCAPE_CHARACTER: SqlState = SqlState(*b"22019");
    /// Data exception: invalid character value for cast [ISO/IEC 9075:1992
    /// §6.10 GR3b i].
    pub const INVALID_CHARACTER_VALUE_FOR_CAST: SqlState = SqlState(*b"22018");
    /// Data exception: substring error [ISO/IEC 9075:1992 §6.7 GR1d].
    pub const SUBSTRING_ERROR: SqlState = SqlState(*b"22011");
    /// Data exception: trim error [ISO/IEC 9075:1992 §6.7 GR5d].
    pub const TRIM_ERROR: SqlState = SqlState(*b"22027");
    /// Data exception: invalid XQuery regular expression [ISO/IEC
    /// 9075-2:2023 §9.28 GR4].
    pub const INVALID_XQUERY_REGULAR_EXPRESSION: SqlState = SqlState(*b"2201S");
    /// Data exception: invalid XQuery option flag [ISO/IEC 9075-2:2023
    /// §9.28 GR5].
    pub const INVALID_XQUERY_OPTION_FLAG: SqlState = SqlState(*b"2201T");
    /// Data exception: attempt to replace a zero-length string [ISO/IEC
    /// 9075-2:2023 §9.29 GR3].
    pub const ATTEMPT_TO_REPLACE_A_ZERO_LENGTH_STRING: SqlState = SqlState(*b"2201U");
    /// Data exception: invalid XQuery replacement string [ISO/IEC
    /// 9075-2:2023 §9.29 GR5].
    pub const INVALID_XQUERY_REPLACEMENT_STRING: SqlState = SqlState(*b"2201V");
    /// Data exception: invalid argument for SQL/JSON datetime function
    /// [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 a, VII 1 c vi].
    pub const INVALID_ARGUMENT_FOR_SQL_JSON_DATETIME_FUNCTION: SqlState = SqlState(*b"22031");
    /// Data exception: non-Boolean SQL/JSON item [ISO/IEC 9075-2:2023
    /// §9.46 GR11g ii 6 C X 1 a].
    pub const NON_BOOLEAN_SQL_JSON_ITEM: SqlState = SqlState(*b"2202V");
    /// Data exception: non-date SQL/JSON item [ISO/IEC 9075-2:2023 §9.46
    /// GR11g ii 6 C XI 1 a].
    pub const NON_DATE_SQL_JSON_ITEM: SqlState = SqlState(*b"2202W");
    /// Data exception: non-string SQL/JSON item [ISO/IEC 9075-2:2023 §9.46
    /// GR11g ii 6 C XV 1 a].
    pub const NON_STRING_SQL_JSON_ITEM: SqlState = SqlState(*b"2202X");
    /// Data exception: non-time SQL/JSON item [ISO/IEC 9075-2:2023 §9.46
    /// GR11g ii 6 C XVI 1 a, XVII 1 a].
    pub const NON_TIME_SQL_JSON_ITEM: SqlState = SqlState(*b"2202Y");
    /// Data exception: non-timestamp SQL/JSON item [ISO/IEC 9075-2:2023
    /// §9.46 GR11g ii 6 C XVIII 1 a, XIX 1 a].
    pub const NON_TIMESTAMP_SQL_JSON_ITEM: SqlState = SqlState(*b"2202Z");
    /// Data exception: invalid JSON text [ISO/IEC 9075-2:2023 §9.42 GR3a
    /// iii 1].
    pub const INVALID_JSON_TEXT: SqlState = SqlState(*b"22032");
    /// Data exception: non-unique keys in a JSON object [ISO/IEC
    /// 9075-2:2023 §9.42 GR3a iii 2].
    pub const NON_UNIQUE_KEYS_IN_A_JSON_OBJECT: SqlState = SqlState(*b"22037");
    /// Data exception: invalid SQL/JSON subscript [ISO/IEC 9075-2:2023
    /// §9.46 GR11g ii 3 E III, GR11g ii 3 F II 2 a].
    pub const INVALID_SQL_JSON_SUBSCRIPT: SqlState = SqlState(*b"22033");
    /// Data exception: non-numeric SQL/JSON item [ISO/IEC 9075-2:2023
    /// §9.46 GR11g ii 6 C III 1 a].
    pub const NON_NUMERIC_SQL_JSON_ITEM: SqlState = SqlState(*b"22036");
    /// Data exception: singleton SQL/JSON item required [ISO/IEC
    /// 9075-2:2023 §9.46 GR11i iii 1, GR11j ii 1].
    pub const SINGLETON_SQL_JSON_ITEM_REQUIRED: SqlState = SqlState(*b"22038");
    /// Data exception: SQL/JSON array not found [ISO/IEC 9075-2:2023 §9.46
    /// GR11g ii 3 B].
    pub const SQL_JSON_ARRAY_NOT_FOUND: SqlState = SqlState(*b"22039");
    /// Data exception: SQL/JSON member not found [ISO/IEC 9075-2:2023 §9.46
    /// GR11g ii 1 B II].
    pub const SQL_JSON_MEMBER_NOT_FOUND: SqlState = SqlState(*b"2203A");
    /// Data exception: SQL/JSON number not found [ISO/IEC 9075-2:2023 §9.46
    /// GR11h ii].
    pub const SQL_JSON_NUMBER_NOT_FOUND: SqlState = SqlState(*b"2203B");
    /// Data exception: SQL/JSON object not found [ISO/IEC 9075-2:2023 §9.46
    /// GR11g ii 2 A II].
    pub const SQL_JSON_OBJECT_NOT_FOUND: SqlState = SqlState(*b"2203C");
    /// Data exception: more than one SQL/JSON item [ISO/IEC 9075-2:2023
    /// §9.44 GR4c iii, §9.48 GR4a, §9.49 GR4c iii].
    pub const MORE_THAN_ONE_SQL_JSON_ITEM: SqlState = SqlState(*b"22034");
    /// Data exception: no SQL/JSON item [ISO/IEC 9075-2:2023 §9.44 GR4c i
    /// 1, §9.48 GR4c i, §9.49 GR4c i 1].
    pub const NO_SQL_JSON_ITEM: SqlState = SqlState(*b"22035");
    /// Data exception: SQL/JSON scalar required [ISO/IEC 9075-2:2023 §9.48
    /// GR4b i].
    pub const SQL_JSON_SCALAR_REQUIRED: SqlState = SqlState(*b"2203F");
    /// Data exception: SQL/JSON item cannot be cast to target type [ISO/IEC
    /// 9075-2:2023 §9.48 GR4b iii 1].
    pub const SQL_JSON_ITEM_CANNOT_BE_CAST_TO_TARGET_TYPE: SqlState = SqlState(*b"2203G");
    /// Data exception: null value not allowed [ISO/IEC 9075-2:2023 §6.34
    /// GR2d ii 1 D, §10.11 GR4b ii 2 B]: the null value as the name of a
    /// member of a JSON object constructed.
    pub const NULL_VALUE_NOT_ALLOWED: SqlState = SqlState(*b"22004");
    /// Data exception: duplicate JSON object key value [ISO/IEC 9075-2:2023
    /// §6.34 GR2d ii 2, §10.11 GR4b ii 3].
    pub const DUPLICATE_JSON_OBJECT_KEY_VALUE: SqlState = SqlState(*b"22030");
    /// Data exception: too many JSON array elements [ISO/IEC 9075-2:2023
    /// §6.34 GR3c ii 2, GR4b ii 2, §10.11 GR5g].
    pub const TOO_MANY_JSON_ARRAY_ELEMENTS: SqlState = SqlState(*b"2203D");
    /// Data exception: too many JSON object members [ISO/IEC 9075-2:2023
    /// §6.34 GR2d ii 3, §10.11 GR4b ii 4].
    pub const TOO_MANY_JSON_OBJECT_MEMBERS: SqlState = SqlState(*b"2203E");
    /// Data exception: not an XML document [ISO/IEC 9075-14:2006 draft
    /// §10.2 GR1a i, §10.15 GR3, §23.1].
    pub const NOT_AN_XML_DOCUMENT: SqlState = SqlState(*b"2200L");
    /// Data exception: not an XQuery document node [ISO/IEC 9075-14:2006
    /// draft §10.2 GR1a ii, §23.1].
    pub const NOT_AN_XQUERY_DOCUMENT_NODE: SqlState = SqlState(*b"2200U");
    /// Data exception: invalid XML document [ISO/IEC 9075-14:2006 draft
    /// §10.16 GR6a i, §23.1].
    pub const INVALID_XML_DOCUMENT: SqlState = SqlState(*b"2200M");
    /// Data exception: invalid XML content [ISO/IEC 9075-14:2006 draft
    /// §10.16 GR6a ii, §23.1].
    pub const INVALID_XML_CONTENT: SqlState = SqlState(*b"2200N");
    /// Data exception: XQuery serialization error [ISO/IEC 9075-14:2006
    /// draft §10.15 GR11, §23.1].
    pub const XQUERY_SERIALIZATION_ERROR: SqlState = SqlState(*b"2200W");
    /// Data exception: invalid XQuery context item [ISO/IEC 9075-14:2006
    /// draft §6.18 GR1a ii 2 B III, §23.1].
    pub const INVALID_XQUERY_CONTEXT_ITEM: SqlState = SqlState(*b"2200V");
    /// Data exception: invalid comment [ISO/IEC 9075-14:2006 draft §6.11
    /// GR3, §23.1].
    pub const INVALID_COMMENT: SqlState = SqlState(*b"2200S");
    /// Data exception: invalid processing instruction [ISO/IEC
    /// 9075-14:2006 draft §6.17 GR4, §23.1].
    pub const INVALID_PROCESSING_INSTRUCTION: SqlState = SqlState(*b"2200T");
    /// Data exception: XML value overflow [ISO/IEC 9075-14:2006 draft §11.2
    /// GR1, §23.1].
    pub const XML_VALUE_OVERFLOW: SqlState = SqlState(*b"2200R");
    /// Data exception: validation failure [ISO/IEC 9075-14:2006 draft
    /// §6.20 GR6a iii 2 D, GR6b iv, §23.1].
    pub const VALIDATION_FAILURE: SqlState = SqlState(*b"2201R");
    /// Data exception: XQuery sequence cannot be validated [ISO/IEC
    /// 9075-14:2006 draft §6.20 GR3c, §23.1].
    pub const XQUERY_SEQUENCE_CANNOT_BE_VALIDATED: SqlState = SqlState(*b"2201J");
    /// Data exception: XQuery document node cannot be validated [ISO/IEC
    /// 9075-14:2006 draft §6.20 GR6a iii 1, §23.1].
    pub const XQUERY_DOCUMENT_NODE_CANNOT_BE_VALIDATED: SqlState = SqlState(*b"2201K");
    /// Data exception: no XML schema found [ISO/IEC 9075-14:2006 draft
    /// §6.20 GR6a iii 2 A II 2, GR6b i 2, §23.1].
    pub const NO_XML_SCHEMA_FOUND: SqlState = SqlState(*b"2201L");
    /// Data exception: no XML element with the specified QName [ISO/IEC
    /// 9075-14:2006 draft §6.20 GR6a iii 2 B I, GR6b ii 1, §23.1].
    pub const NO_XML_ELEMENT_WITH_THE_SPECIFIED_QNAME: SqlState = SqlState(*b"2201P");
    /// Data exception: no XML element with the specified namespace
    /// [ISO/IEC 9075-14:2006 draft §6.20 GR6a iii 2 B II, GR6b ii 2,
    /// §23.1].
    pub const NO_XML_ELEMENT_WITH_THE_SPECIFIED_NAMESPACE: SqlState = SqlState(*b"2201Q");
    /// Data exception: element namespace not declared [ISO/IEC
    /// 9075-14:2006 draft §11.6 GR1a, §23.1].
    pub const ELEMENT_NAMESPACE_NOT_DECLARED: SqlState = SqlState(*b"2201M");
    /// Data exception: global element not declared [ISO/IEC 9075-14:2006
    /// draft §11.6 GR1b, §23.1].
    pub const GLOBAL_ELEMENT_NOT_DECLARED: SqlState = SqlState(*b"2201N");
    /// XQuery error: a static error, type error or dynamic error XQuery
    /// raises [ISO/IEC 9075-14:2006 draft §3.1.3.56, §6.6 GR3d vi, GR4j,
    /// §6.13 GR8, §6.18 GR3, §23.1].
    pub const XQUERY_ERROR: SqlState = SqlState(*b"10000");
    /// SQL/XML mapping error: invalid XML character [ISO/IEC 9075-14:2006
    /// draft §9.8 GR9a ii, §23.1].
    pub const INVALID_XML_CHARACTER: SqlState = SqlState(*b"0N002");
    /// Data exception: sequence generator limit exceeded [ISO/IEC
    /// 9075-2:2023 §9.35 GR3a].
    pub const SEQUENCE_GENERATOR_LIMIT_EXCEEDED: SqlState = SqlState(*b"2200H");
    /// Data exception: array element error [ISO/IEC 9075-2:2023 §6.25
    /// GR2b].
    pub const ARRAY_ELEMENT_ERROR: SqlState = SqlState(*b"2202E");
    /// Data exception: array data, right truncation [ISO/IEC 9075-2:2023
    /// §9.2 GR3b xxii 2B].
    pub const ARRAY_DATA_RIGHT_TRUNCATION: SqlState = SqlState(*b"2202F");
    /// Data exception: multiset value overflow [ISO/IEC 9075-2:2023
    /// §10.9 GR3b].
    pub const MULTISET_VALUE_OVERFLOW: SqlState = SqlState(*b"2200Q");
    /// [ISO/IEC 9075-4:2006 draft §14.5 GR4a] Data exception: null value in
    /// field reference.
    pub const NULL_VALUE_IN_FIELD_REFERENCE: SqlState = SqlState(*b"2202A");
    /// Data exception: null value in array target [ISO/IEC 9075-2:2023
    /// §15.5 GR3b i 1].
    pub const NULL_VALUE_IN_ARRAY_TARGET: SqlState = SqlState(*b"2200E");
    /// Data exception: indicator overflow.
    pub const INDICATOR_OVERFLOW: SqlState = SqlState(*b"22022");
    /// Data exception: invalid escape sequence.
    pub const INVALID_ESCAPE_SEQUENCE: SqlState = SqlState(*b"22025");
    /// [ISO/IEC 9075-2:2023 §8.5 GR4d i 1] Data exception: invalid escape
    /// octet.
    pub const INVALID_ESCAPE_OCTET: SqlState = SqlState(*b"2200D");
    /// Data exception: invalid datetime format.
    pub const INVALID_DATETIME_FORMAT: SqlState = SqlState(*b"22007");
    /// Data exception: datetime field overflow.
    pub const DATETIME_FIELD_OVERFLOW: SqlState = SqlState(*b"22008");
    /// Data exception: invalid time zone displacement value.
    pub const INVALID_TIME_ZONE_DISPLACEMENT: SqlState = SqlState(*b"22009");
    /// Data exception: interval field overflow.
    pub const INTERVAL_FIELD_OVERFLOW: SqlState = SqlState(*b"22015");
    /// Integrity constraint violation.
    pub const INTEGRITY_CONSTRAINT_VIOLATION: SqlState = SqlState(*b"23000");
    /// Integrity constraint violation: restrict violation [ISO/IEC
    /// 9075-2:2023 §15.18 GR9a iv, GR10a iv].
    pub const RESTRICT_VIOLATION: SqlState = SqlState(*b"23001");
    /// \[EXTENSION\] Integrity constraint violation: no partition holds
    /// the row, of a partitioned table, or the partition written does not
    /// (RULES.md D155).
    pub const NO_PARTITION: SqlState = SqlState(*b"23T01");
    /// Triggered data change violation.
    pub const TRIGGERED_DATA_CHANGE_VIOLATION: SqlState = SqlState(*b"27000");
    /// Triggered action exception [ISO/IEC 9075-2:2023 §15.21 GR5b i 1,
    /// GR6m].
    pub const TRIGGERED_ACTION_EXCEPTION: SqlState = SqlState(*b"09000");
    /// Prohibited statement encountered during trigger execution [ISO/IEC
    /// 9075-2:2023 §15.21 GR6e iii, GR6h].
    pub const PROHIBITED_STATEMENT_IN_TRIGGER: SqlState = SqlState(*b"0W000");
    /// SQL routine exception [ISO/IEC 9075-2:2023 §24.1, Table 41].
    pub const SQL_ROUTINE_EXCEPTION: SqlState = SqlState(*b"2F000");
    /// SQL routine exception: modifying SQL-data not permitted [ISO/IEC
    /// 9075-2:2023 §9.18 GR8b i].
    pub const MODIFYING_DATA_NOT_PERMITTED: SqlState = SqlState(*b"2F002");
    /// SQL routine exception: prohibited SQL-statement attempted [ISO/IEC
    /// 9075-2:2023 §9.18 GR8b iii to vi].
    pub const PROHIBITED_ROUTINE_STATEMENT: SqlState = SqlState(*b"2F003");
    /// SQL routine exception: reading SQL-data not permitted [ISO/IEC
    /// 9075-2:2023 §9.18 GR8b ii].
    pub const READING_DATA_NOT_PERMITTED: SqlState = SqlState(*b"2F004");
    /// SQL routine exception: function executed no return statement
    /// [ISO/IEC 9075-2:2023 §9.18 GR8b ix].
    pub const NO_RETURN_STATEMENT: SqlState = SqlState(*b"2F005");
    /// SQL routine exception: invalid search expression [ISO/IEC
    /// 13249-2:2002 (CD, SC32 N0640) §11].
    pub const INVALID_SEARCH_EXPRESSION: SqlState = SqlState(*b"2FF01");
    /// SQL routine exception: invalid language specification [ISO/IEC
    /// 13249-2:2002 (CD, SC32 N0640) §11].
    pub const INVALID_LANGUAGE_SPECIFICATION: SqlState = SqlState(*b"2FF02");
    /// SQL routine exception: effectively empty search expression [ISO/IEC
    /// 13249-2:2002 (CD, SC32 N0640) §11].
    pub const EFFECTIVELY_EMPTY_SEARCH_EXPRESSION: SqlState = SqlState(*b"2FF03");
    /// Invalid cursor state.
    pub const INVALID_CURSOR_STATE: SqlState = SqlState(*b"24000");
    /// Invalid transaction state.
    pub const INVALID_TRANSACTION_STATE: SqlState = SqlState(*b"25000");
    /// Invalid transaction termination [ISO/IEC 9075-2:2023 §17.7 GR2,
    /// §17.8 GR2].
    pub const INVALID_TRANSACTION_TERMINATION: SqlState = SqlState(*b"2D000");
    /// Savepoint exception [ISO/IEC 9075-2:2023 §24.1, Table 41].
    pub const SAVEPOINT_EXCEPTION: SqlState = SqlState(*b"3B000");
    /// Savepoint exception: invalid specification [ISO/IEC 9075-2:2023
    /// §17.6 GR2, §17.8 GR3b, GR3c].
    pub const SAVEPOINT_INVALID_SPECIFICATION: SqlState = SqlState(*b"3B001");
    /// Savepoint exception: too many [ISO/IEC 9075-2:2023 §17.5 GR3].
    pub const SAVEPOINT_TOO_MANY: SqlState = SqlState(*b"3B002");
    /// Invalid transaction state: active SQL-transaction [ISO/IEC
    /// 9075-2:2023 §19.3 GR1].
    pub const ACTIVE_TRANSACTION: SqlState = SqlState(*b"25001");
    /// Invalid transaction state: read-only SQL-transaction [ISO/IEC
    /// 9075-2:2023 §14.10 GR1].
    pub const READ_ONLY_TRANSACTION: SqlState = SqlState(*b"25006");
    /// Cursor sensitivity exception: request failed [ISO/IEC 9075-2:2023
    /// §14.10 GR2, GR3].
    pub const CURSOR_SENSITIVITY_FAILED: SqlState = SqlState(*b"36002");
    /// Invalid condition number.
    pub const INVALID_CONDITION_NUMBER: SqlState = SqlState(*b"35000");
    /// Invalid catalog name.
    pub const INVALID_CATALOG_NAME: SqlState = SqlState(*b"3D000");
    /// Invalid schema name.
    pub const INVALID_SCHEMA_NAME: SqlState = SqlState(*b"3F000");
    /// Syntax error or access rule violation in direct SQL statement.
    pub const SYNTAX_ERROR_IN_DIRECT_STATEMENT: SqlState = SqlState(*b"2A000");
    /// Connection exception: transaction resolution unknown [ISO/IEC
    /// 9075:1992 §4.29]: whether the transaction committed cannot be
    /// verified. \[DESIGN\] Of a commit in doubt (RULES.md D25).
    pub const TRANSACTION_RESOLUTION_UNKNOWN: SqlState = SqlState(*b"08007");
    /// Transaction rollback.
    pub const TRANSACTION_ROLLBACK: SqlState = SqlState(*b"40000");
    /// Transaction rollback: serialization failure.
    pub const SERIALIZATION_FAILURE: SqlState = SqlState(*b"40001");
    /// Transaction rollback: integrity constraint violation [ISO/IEC
    /// 9075:1992 §14.3 GR5a].
    pub const TRANSACTION_ROLLBACK_INTEGRITY: SqlState = SqlState(*b"40002");
    /// With check option violation.
    pub const WITH_CHECK_OPTION_VIOLATION: SqlState = SqlState(*b"44000");
    /// \[DESIGN\] A statement refused by a conflict with another
    /// transaction, undone while its transaction goes on, so not a
    /// transaction rollback.
    pub const STATEMENT_CONFLICT: SqlState = SqlState(*b"TB911");
    /// \[DESIGN\] Use refused by the transaction policy.
    pub const POLICY: SqlState = SqlState(*b"TB912");
    /// \[DESIGN\] A table damaged by a change that could not be undone.
    pub const DAMAGED: SqlState = SqlState(*b"TB914");
    /// \[DESIGN\] A file could not be read, written or synced.
    pub const IO: SqlState = SqlState(*b"TB915");
    /// \[DESIGN\] Stored data failed its checksum or format.
    pub const CORRUPT: SqlState = SqlState(*b"TB916");
    /// \[DESIGN\] The host (Rust) API was misused.
    pub const API: SqlState = SqlState(*b"TB910");
    /// \[DESIGN\] The memory budget is spent (SQLCODE -918).
    pub const RESOURCES: SqlState = SqlState(*b"TB918");
    /// \[DESIGN\] The statement was cancelled (SQLCODE -920).
    pub const CANCELLED: SqlState = SqlState(*b"TB920");
    /// \[EXTENSION\] A backup or archived object failed its checksum or
    /// format (SQLCODE -916, RULES.md D161).
    pub const BACKUP_DAMAGED: SqlState = SqlState(*b"TBB01");
    /// \[EXTENSION\] A restore's recovery target was not met in the
    /// records it had (SQLCODE -910, RULES.md D164).
    pub const RECOVERY_TARGET_NOT_REACHED: SqlState = SqlState(*b"TBB02");

    /// [ISO/IEC 9075-4:2006 draft §8.2 SR1, SR2] The SQLSTATE value of the
    /// `<sqlstate value>` `s`: five characters, each a digit or a simple
    /// Latin upper case letter, its class not 00, successful completion.
    /// None if it is not one.
    pub fn parse(s: &str) -> Option<SqlState> {
        let bytes: [u8; 5] = s.as_bytes().try_into().ok()?;
        let valid = bytes.iter().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase());
        let state = SqlState(bytes);
        (valid && state.class() != SqlState::SUCCESSFUL_COMPLETION.class()).then_some(state)
    }

    /// The five characters.
    pub fn as_str(&self) -> &str {
        // Every value is made of ASCII digits and letters (`parse`).
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// The class: the first two characters.
    pub fn class(&self) -> &str {
        self.as_str().get(..2).unwrap_or_default()
    }

    /// Whether it is of a class of syntax error or access rule violation
    /// [ISO/IEC 9075:1992 §18.1 GR3g]: 42, 2A (in a direct SQL statement)
    /// or 37 (in a dynamic SQL statement) [§22.1].
    pub fn is_syntax_or_access(self) -> bool {
        /// [§22.1] The class of syntax error or access rule violation.
        const SYNTAX_OR_ACCESS: &str = "42";
        [SYNTAX_OR_ACCESS, SqlState::SYNTAX_ERROR_IN_DIRECT_STATEMENT.class(), SqlState::DYNAMIC_SYNTAX_ERROR.class()]
            .contains(&self.class())
    }

    /// The state an error of `code` has, unless raised with a closer one:
    /// the class of its condition, without a subclass.
    pub fn of(code: Code) -> SqlState {
        match code {
            Code::Lexical | Code::Syntax | Code::SyntaxRule | Code::Privilege => SqlState::SYNTAX_ERROR_IN_DIRECT_STATEMENT,
            Code::Constraint => SqlState::INTEGRITY_CONSTRAINT_VIOLATION,
            Code::Data => SqlState::DATA_EXCEPTION,
            Code::Cursor => SqlState::INVALID_CURSOR_STATE,
            Code::Signalled => SqlState::UNHANDLED_USER_EXCEPTION,
            Code::Cardinality => SqlState::CARDINALITY_VIOLATION,
            Code::Conflict => SqlState::STATEMENT_CONFLICT,
            Code::Policy => SqlState::POLICY,
            Code::TransactionState => SqlState::INVALID_TRANSACTION_STATE,
            Code::Resources => SqlState::RESOURCES,
            Code::Connection => SqlState::CONNECTION_EXCEPTION,
            Code::Cancelled => SqlState::CANCELLED,
            Code::Dynamic => SqlState::DYNAMIC_SQL_ERROR,
            Code::InDoubt => SqlState::TRANSACTION_RESOLUTION_UNKNOWN,
            Code::CommitFailed => SqlState::TRANSACTION_ROLLBACK,
            Code::Damaged => SqlState::DAMAGED,
            Code::Io => SqlState::IO,
            Code::Corrupt => SqlState::CORRUPT,
            Code::Unsupported => SqlState::FEATURE_NOT_SUPPORTED,
            Code::Api => SqlState::API,
        }
    }
}

impl fmt::Display for SqlState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
