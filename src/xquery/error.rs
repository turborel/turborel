//! The errors XQuery names that a query's text alone shows [XQuery §2.3.1,
//! Appendix F], raised as SQL/XML's exception condition XQuery error
//! [ISO/IEC 9075-14:2006 draft §6.18 GR3], and where in the text each is.

mod codes;

use std::fmt;

use crate::error::{Code, SqlError, SqlState};

/// An error code of \[XQuery\]: the local part of the error's QName in the
/// namespace `http://www.w3.org/2005/xqt-errors`, as Appendix F prints it
/// [XQuery §2.3.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode(&'static str);

impl ErrorCode {
    /// [XQuery Appendix F] An expression is not an instance of the
    /// grammar.
    pub const XPST0003: ErrorCode = ErrorCode("XPST0003");
    /// [XQuery Appendix F] A namespace declaration attribute's value is not
    /// a URILiteral.
    pub const XQST0022: ErrorCode = ErrorCode("XQST0022");
    /// [XQuery Appendix F] A version declaration names a version the
    /// implementation does not support.
    pub const XQST0031: ErrorCode = ErrorCode("XQST0031");
    /// [XQuery Appendix F] A prolog has more than one base URI declaration.
    pub const XQST0032: ErrorCode = ErrorCode("XQST0032");
    /// [XQuery Appendix F] A prolog declares one namespace prefix more
    /// than once.
    pub const XQST0033: ErrorCode = ErrorCode("XQST0033");
    /// [XQuery Appendix F] A prolog has more than one default collation
    /// declaration.
    pub const XQST0038: ErrorCode = ErrorCode("XQST0038");
    /// [XQuery Appendix F] A function declaration has two parameters of
    /// one name.
    pub const XQST0039: ErrorCode = ErrorCode("XQST0039");
    /// [XQuery Appendix F] A direct element constructor's attributes do
    /// not have distinct names.
    pub const XQST0040: ErrorCode = ErrorCode("XQST0040");
    /// [XQuery Appendix F] A prolog has more than one copy-namespaces
    /// declaration.
    pub const XQST0055: ErrorCode = ErrorCode("XQST0055");
    /// [XQuery Appendix F] A schema import binds a prefix to no target
    /// namespace.
    pub const XQST0057: ErrorCode = ErrorCode("XQST0057");
    /// [XQuery Appendix F] A prolog has more than one ordering mode
    /// declaration.
    pub const XQST0065: ErrorCode = ErrorCode("XQST0065");
    /// [XQuery Appendix F] A prolog has more than one default element/type
    /// or more than one default function namespace declaration.
    pub const XQST0066: ErrorCode = ErrorCode("XQST0066");
    /// [XQuery Appendix F] A prolog has more than one construction
    /// declaration.
    pub const XQST0067: ErrorCode = ErrorCode("XQST0067");
    /// [XQuery Appendix F] A prolog has more than one boundary-space
    /// declaration.
    pub const XQST0068: ErrorCode = ErrorCode("XQST0068");
    /// [XQuery Appendix F] A prolog has more than one empty order
    /// declaration.
    pub const XQST0069: ErrorCode = ErrorCode("XQST0069");
    /// [XQuery Appendix F] The prefix `xml` or `xmlns` is declared again,
    /// or another prefix is bound to the namespace of `xml`.
    pub const XQST0070: ErrorCode = ErrorCode("XQST0070");
    /// [XQuery Appendix F] A direct element constructor's namespace
    /// declaration attributes do not have distinct names.
    pub const XQST0071: ErrorCode = ErrorCode("XQST0071");
    /// [XQuery Appendix F] A namespace declaration attribute undeclares a
    /// prefix, and the implementation does not support Namespaces in XML
    /// 1.1.
    pub const XQST0085: ErrorCode = ErrorCode("XQST0085");
    /// [XQuery Appendix F] A version declaration's encoding is no
    /// `EncName`.
    pub const XQST0087: ErrorCode = ErrorCode("XQST0087");
    /// [XQuery Appendix F] A module import's target namespace is of zero
    /// length.
    pub const XQST0088: ErrorCode = ErrorCode("XQST0088");
    /// [XQuery Appendix F] The schemas a prolog imports are not valid
    /// together.
    pub const XQST0012: ErrorCode = ErrorCode("XQST0012");
    /// [XQuery Appendix F] Two schema components imported define one name
    /// in one symbol space.
    pub const XQST0035: ErrorCode = ErrorCode("XQST0035");
    /// [XQuery Appendix F] Two schema imports of a prolog specify one target
    /// namespace.
    pub const XQST0058: ErrorCode = ErrorCode("XQST0058");
    /// [XQuery Appendix F] No schema or module of the target namespace a
    /// schema or module import specifies is found.
    pub const XQST0059: ErrorCode = ErrorCode("XQST0059");

    /// The code as Appendix F prints it, without its prefix.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "err:{}", self.0)
    }
}

/// Where a character of a query's text is: its line and column, each
/// counted from 1, line ends normalized [XQuery Appendix A.2.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Place {
    /// The line.
    pub(super) line: usize,
    /// The column, in characters.
    pub(super) column: usize,
}

impl Place {
    /// Where the character after `before` is.
    pub(super) fn after(before: &[char]) -> Place {
        let line = before.iter().filter(|&&c| c == '\n').count().saturating_add(1);
        let column = before.iter().rev().take_while(|&&c| c != '\n').count().saturating_add(1);
        Place { line, column }
    }
}

/// The XQuery error `code` at `place`, `why` saying which rule of
/// \[XQuery\] the text breaks: SQL/XML's exception condition XQuery error
/// [ISO/IEC 9075-14:2006 draft §6.18 GR3].
pub(super) fn xquery_error(code: ErrorCode, place: Place, why: &str) -> SqlError {
    SqlError::new(
        Code::Data,
        format!("[ISO/IEC 9075-14:2006 draft §6.18 GR3] XQuery error {code} at line {}, column {}: {why}", place.line, place.column),
    )
    .with_state(SqlState::XQUERY_ERROR)
}

/// The XQuery error `code` raised as a query is analysed in its static
/// context or evaluated, `why` saying which rule of \[XQuery\] or
/// \[XQueryFO\] it breaks: SQL/XML's exception condition XQuery error
/// [ISO/IEC 9075-14:2006 draft §6.18 GR3], its message naming the code.
#[must_use]
pub fn error(code: ErrorCode, why: &str) -> SqlError {
    named_error(&code.to_string(), why)
}

/// The XQuery error whose QName is written `name`, `err:` and its code of
/// the namespace of \[XQuery\]'s errors or the URI `NS#LP` of another
/// [XQueryFO §3], as fn:error raises one a query names: SQL/XML's
/// exception condition XQuery error [ISO/IEC 9075-14:2006 draft §6.18
/// GR3].
pub(super) fn named_error(name: &str, why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §6.18 GR3] XQuery error {name}: {why}"))
        .with_state(SqlState::XQUERY_ERROR)
}

/// The error of a query whose evaluation passes one of turborel's limits,
/// `why` (RULES.md D18, D106): SQLCODE -900.
pub(super) fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the XQuery evaluation is beyond turborel's limits: {why}"))
}

/// The error of a query nested deeper than `MAX_NESTING`, at `place`
/// (RULES.md D18, R-CF-6).
pub(super) fn too_deep(place: Place) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!(
            "[DESIGN] the query nests deeper than turborel's limit of {} levels at line {}, column {}",
            super::MAX_NESTING,
            place.line,
            place.column
        ),
    )
}
