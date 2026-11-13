//! [X3.135-1986 §5.3] The `<delimiter token>`s other than `<character
//! string literal>`, one constant each.

/// The not equals operator.
pub const NOT_EQUALS: &str = "<>";
/// The greater than or equals operator.
pub const GREATER_OR_EQUAL: &str = ">=";
/// The less than or equals operator.
pub const LESS_OR_EQUAL: &str = "<=";
/// Comma.
pub const COMMA: &str = ",";
/// Left parenthesis.
pub const LEFT_PAREN: &str = "(";
/// Right parenthesis.
pub const RIGHT_PAREN: &str = ")";
/// The less than operator.
pub const LESS: &str = "<";
/// The greater than operator.
pub const GREATER: &str = ">";
/// Period.
pub const PERIOD: &str = ".";
/// Colon.
pub const COLON: &str = ":";
/// The equals operator.
pub const EQUALS: &str = "=";
/// Asterisk.
pub const ASTERISK: &str = "*";
/// Plus sign.
pub const PLUS: &str = "+";
/// Minus sign.
pub const MINUS: &str = "-";
/// Solidus.
pub const SOLIDUS: &str = "/";
/// [ISO/IEC 9075:1992 §5.2] The concatenation operator.
pub const CONCATENATION: &str = "||";
/// [ISO/IEC 9075:1992 §5.1] The question mark: an `<SQL special
/// character>`, so a `<delimiter token>` [§5.2], and the `<dynamic
/// parameter specification>` [§6.2].
pub const QUESTION: &str = "?";
/// [ISO/IEC 9075:1992 §5.1, §20.1] The semicolon: after a direct SQL
/// statement.
pub const SEMICOLON: &str = ";";
/// [ISO/IEC 9075-2:2023 §5.1] `<left bracket or trigraph>`: of an
/// array type, an array element and an array value constructor.
pub const LEFT_BRACKET: &str = "[";
/// [ISO/IEC 9075-2:2023 §5.1] `<right bracket or trigraph>`.
pub const RIGHT_BRACKET: &str = "]";
