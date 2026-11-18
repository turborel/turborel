//! [X3.135-1986 §5.3] The `<delimiter token>`s other than `<character
//! string literal>`, one constant each.

/// The not equals operator.
pub const NOT_EQUALS: &str = "<>";
/// The greater than or equals operator.
pub const GREATER_OR_EQUAL: &str = ">=";
/// The less than or equals operator.
pub const LESS_OR_EQUAL: &str = "<=";
/// [ISO/IEC 9075-2:2023 §5.2] `<right arrow>`: the `<dereference
/// operator>` [§6.21].
pub const RIGHT_ARROW: &str = "->";
/// [ISO/IEC 9075-2:2023 §5.2] `<double colon>`: of a `<static method
/// invocation>` [§6.19].
pub const DOUBLE_COLON: &str = "::";
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
/// [ISO/IEC 9075-2:2023 §5.2] `<left brace minus>`: the start of a row
/// pattern's rows left out of ALL ROWS PER MATCH's output [§7.9].
pub const LEFT_BRACE_MINUS: &str = "{-";
/// [ISO/IEC 9075-2:2023 §5.2] `<right minus brace>`: their end.
pub const RIGHT_MINUS_BRACE: &str = "-}";
/// [ISO/IEC 9075-2:2023 §5.1] `<vertical bar>`: of a row pattern
/// alternation [§7.9].
pub const VERTICAL_BAR: &str = "|";
/// [ISO/IEC 9075-2:2023 §5.1] `<left brace>`: of a row pattern
/// quantifier's bounds [§7.9].
pub const LEFT_BRACE: &str = "{";
/// [ISO/IEC 9075-2:2023 §5.1] `<right brace>`.
pub const RIGHT_BRACE: &str = "}";
/// [ISO/IEC 9075-2:2023 §5.1] `<circumflex>`: the start of a row pattern
/// partition [§7.9].
pub const CIRCUMFLEX: &str = "^";
/// [ISO/IEC 9075-2:2023 §5.1] `<dollar sign>`: the end of a row pattern
/// partition [§7.9].
pub const DOLLAR_SIGN: &str = "$";
/// \[EXTENSION\] The distance operators of VECTOR (RULES.md D88), each
/// a call of VECTOR_DISTANCE by its metric (`vector::functions::OPERATORS`):
/// EUCLIDEAN.
pub const EUCLIDEAN_DISTANCE: &str = "<->";
/// \[EXTENSION\] COSINE.
pub const COSINE_DISTANCE: &str = "<=>";
/// \[EXTENSION\] DOT, negated.
pub const DOT_DISTANCE: &str = "<#>";
/// \[EXTENSION\] MANHATTAN.
pub const MANHATTAN_DISTANCE: &str = "<+>";
/// \[EXTENSION\] HAMMING.
pub const HAMMING_DISTANCE: &str = "<~>";
/// \[EXTENSION\] JACCARD.
pub const JACCARD_DISTANCE: &str = "<%>";
