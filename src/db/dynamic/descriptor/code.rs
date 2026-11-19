//! [ISO/IEC 9075:1992 §17.1 Table 18] The codes of the data types.

/// CHARACTER.
pub(in crate::db::dynamic) const CHARACTER: i64 = 1;
/// NUMERIC.
pub(in crate::db::dynamic) const NUMERIC: i64 = 2;
/// DECIMAL.
pub(in crate::db::dynamic) const DECIMAL: i64 = 3;
/// INTEGER.
pub(in crate::db::dynamic) const INTEGER: i64 = 4;
/// SMALLINT.
pub(in crate::db::dynamic) const SMALLINT: i64 = 5;
/// FLOAT.
pub(in crate::db::dynamic) const FLOAT: i64 = 6;
/// REAL.
pub(in crate::db::dynamic) const REAL: i64 = 7;
/// DOUBLE PRECISION.
pub(in crate::db::dynamic) const DOUBLE_PRECISION: i64 = 8;
/// DATE, TIME or TIMESTAMP.
pub(in crate::db::dynamic) const DATETIME: i64 = 9;
/// INTERVAL.
pub(in crate::db::dynamic) const INTERVAL: i64 = 10;
/// CHARACTER VARYING.
pub(in crate::db::dynamic) const VARYING: i64 = 12;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] BOOLEAN.
pub(in crate::db::dynamic) const BOOLEAN: i64 = 16;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] BIGINT.
pub(in crate::db::dynamic) const BIGINT: i64 = 25;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] DECFLOAT.
pub(in crate::db::dynamic) const DECFLOAT: i64 = 26;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] BINARY.
pub(in crate::db::dynamic) const BINARY: i64 = 60;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] BINARY VARYING.
pub(in crate::db::dynamic) const BINARY_VARYING: i64 = 61;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] BINARY LARGE OBJECT.
pub(in crate::db::dynamic) const BLOB: i64 = 30;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] BINARY LARGE OBJECT LOCATOR.
pub(in crate::db::dynamic) const BLOB_LOCATOR: i64 = 31;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] CHARACTER LARGE OBJECT.
pub(in crate::db::dynamic) const CLOB: i64 = 40;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] CHARACTER LARGE OBJECT
/// LOCATOR.
pub(in crate::db::dynamic) const CLOB_LOCATOR: i64 = 41;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] ROW TYPE.
pub(in crate::db::dynamic) const ROW: i64 = 19;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] A user-defined type.
pub(in crate::db::dynamic) const USER_DEFINED: i64 = 17;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] ARRAY.
pub(in crate::db::dynamic) const ARRAY: i64 = 50;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] MULTISET.
pub(in crate::db::dynamic) const MULTISET: i64 = 55;
/// [ISO/IEC 9075-2:2023 §20.1, Table 30] REF, a reference type.
pub(in crate::db::dynamic) const REF: i64 = 20;
/// [ISO/IEC 9075-2:2023 §20.1, Table 34] DISTINCT, the category of a
/// distinct type.
pub(in crate::db::dynamic) const DISTINCT: i64 = 1;
/// [ISO/IEC 9075-2:2023 §20.1, Table 34] STRUCTURED, the category of a
/// structured type.
pub(in crate::db::dynamic) const STRUCTURED: i64 = 2;
/// \[DESIGN\] An implementation-defined data type, of a code less than 0:
/// a plug-in's (RULES.md D53), each described by this one code.
pub(in crate::db::dynamic) const EXTENSION: i64 = -1;
/// [ISO/IEC 9075-14:2006 draft §17.1 GR1, Table 12] XML.
pub(in crate::db::dynamic) const XML: i64 = 137;
