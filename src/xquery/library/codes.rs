//! The errors the library raises [XQueryFO Appendix C] [XQuery Appendix
//! F], each a QName of the namespace `http://www.w3.org/2005/xqt-errors`
//! [XQueryFO §3], raised as SQL/XML's exception condition XQuery error
//! [ISO/IEC 9075-14:2006 draft §6.18 GR3], SQLSTATE 10000, the code in its
//! message.

use crate::error::{Code, SqlError, SqlState};

/// [XQueryFO Appendix C] Unidentified error.
pub const FOER0000: &str = "FOER0000";
/// [XQueryFO Appendix C] Division by zero.
pub const FOAR0001: &str = "FOAR0001";
/// [XQueryFO Appendix C] Numeric operation overflow/underflow.
pub const FOAR0002: &str = "FOAR0002";
/// [XQueryFO Appendix C] Invalid lexical value.
pub const FOCA0002: &str = "FOCA0002";
/// [XQueryFO Appendix C] Input value too large for integer.
pub const FOCA0003: &str = "FOCA0003";
/// [XQueryFO Appendix C] NaN supplied as float/double value.
pub const FOCA0005: &str = "FOCA0005";
/// [XQueryFO Appendix C] Code point not valid.
pub const FOCH0001: &str = "FOCH0001";
/// [XQueryFO Appendix C] Unsupported collation.
pub const FOCH0002: &str = "FOCH0002";
/// [XQueryFO Appendix C] Unsupported normalization form.
pub const FOCH0003: &str = "FOCH0003";
/// [XQueryFO Appendix C] Collation does not support collation units.
pub const FOCH0004: &str = "FOCH0004";
/// [XQueryFO Appendix C] No context document.
pub const FODC0001: &str = "FODC0001";
/// [XQueryFO Appendix C] Error retrieving resource.
pub const FODC0002: &str = "FODC0002";
/// [XQueryFO Appendix C] Invalid argument to fn:collection().
pub const FODC0004: &str = "FODC0004";
/// [XQueryFO Appendix C] Invalid argument to fn:doc.
pub const FODC0005: &str = "FODC0005";
/// [XQueryFO Appendix C] Overflow/underflow in date/time operation.
pub const FODT0001: &str = "FODT0001";
/// [XQueryFO Appendix C] Overflow/underflow in duration operation.
pub const FODT0002: &str = "FODT0002";
/// [XQueryFO Appendix C] Invalid timezone value.
pub const FODT0003: &str = "FODT0003";
/// [XQueryFO Appendix C] Undefined context item.
pub const FONC0001: &str = "FONC0001";
/// [XQueryFO Appendix C] No namespace found for prefix.
pub const FONS0004: &str = "FONS0004";
/// [XQueryFO Appendix C] Base-uri not defined in the static context.
pub const FONS0005: &str = "FONS0005";
/// [XQueryFO Appendix C] Invalid value for cast/constructor.
pub const FORG0001: &str = "FORG0001";
/// [XQueryFO Appendix C] fn:zero-or-one called with a sequence
/// containing more than one item.
pub const FORG0003: &str = "FORG0003";
/// [XQueryFO Appendix C] fn:one-or-more called with a sequence containing
/// no items.
pub const FORG0004: &str = "FORG0004";
/// [XQueryFO Appendix C] fn:exactly-one called with a sequence containing
/// zero or more than one item.
pub const FORG0005: &str = "FORG0005";
/// [XQueryFO Appendix C] Invalid argument type.
pub const FORG0006: &str = "FORG0006";
/// [XQueryFO Appendix C] Both arguments to fn:dateTime have a specified
/// timezone.
pub const FORG0008: &str = "FORG0008";
/// [XQueryFO Appendix C] Error in resolving a relative URI against a base
/// URI in fn:resolve-uri.
pub const FORG0009: &str = "FORG0009";
/// [XQueryFO Appendix C] Invalid regular expression flags.
pub const FORX0001: &str = "FORX0001";
/// [XQueryFO Appendix C] Invalid regular expression.
pub const FORX0002: &str = "FORX0002";
/// [XQueryFO Appendix C] Regular expression matches zero-length string.
pub const FORX0003: &str = "FORX0003";
/// [XQueryFO Appendix C] Invalid replacement string.
pub const FORX0004: &str = "FORX0004";
/// [XQueryFO Appendix C] Type error. Context item is not a node.
pub const FOTY0011: &str = "FOTY0011";
/// [XQuery Appendix F] A value does not match a required type, as an
/// argument of a function its parameter's [XQuery §3.1.5].
pub const XPTY0004: &str = "XPTY0004";

/// The XQuery error `code`, `why` saying what raised it and where
/// \[XQueryFO\] says so: SQLSTATE 10000 [ISO/IEC 9075-14:2006 draft §6.18
/// GR3].
pub fn fail(code: &str, why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §6.18 GR3] XQuery error err:{code}: {why}"))
        .with_state(SqlState::XQUERY_ERROR)
}

/// The error of a value past turborel's limits, `why` (RULES.md D18, D86):
/// SQLCODE -900.
pub fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the XML value is beyond turborel's limits: {why}"))
}
