//! The SQL/JSON path language [ISO/IEC 9075-2:2023 §9.45, §9.46]: the
//! text of a path expression read to its syntax tree (`json/path/ast.rs`).
//!
//! [§9.45 SR2] The text is split into tokens by the lexical rules of
//! ECMAScript 5.1 the standard adopts, as it modifies them
//! (`json/path/lex.rs`). The tokens are parsed by the Format of §9.46 and
//! its Syntax Rules 3 and 5 to 7 (`json/path/parse.rs`, and
//! `json/path/parse/predicate.rs` for predicates). Syntax Rules 4 and 8
//! need the PASSING clause, and are not checked here.
//!
//! [§9.45 SR2d] No word is reserved: the lexer yields every word as an
//! identifier, and the parser takes it as a key word only where the Format
//! has one. A `<JSON path key name>` is never followed by `(` [NOTE 483],
//! so `$.type` is the member `type`, `$.type()` the item method.
//!
//! [§9.46 GR] A parsed path is evaluated of a context item and the values
//! of its named variables to an SQL/JSON sequence (`json/path/eval.rs`,
//! `Path::evaluate`).

mod ast;
mod eval;
mod lex;
mod parse;

pub use ast::{Accessor, Comparison, Expr, Method, Mode, Operator, Path, Predicate, Sign, Subscript};
pub use parse::parse_path;

use crate::error::{Code, SqlError};

/// \[DESIGN\] How deep a path expression nests at most: parentheses,
/// filters, subscripts, signs and negations around what is read, and the
/// nodes of its syntax tree, so the parser's recursion and every walk of
/// the tree are bounded (RULES.md R-CF-6).
const MAX_NESTING: usize = 64;

/// [§9.45 SR4] What a `<JSON path identifier>` that is a variable begins
/// with, the `<dollar sign>`.
const DOLLAR: char = '$';

// [§9.45 F] The `<SQL/JSON key word>`s the parser reads.
/// `abs`.
const ABS: &str = "abs";
/// `ceiling`.
const CEILING: &str = "ceiling";
/// `double`.
const DOUBLE: &str = "double";
/// `exists`.
const EXISTS: &str = "exists";
/// `false`.
const FALSE: &str = "false";
/// `flag`.
const FLAG: &str = "flag";
/// `floor`.
const FLOOR: &str = "floor";
/// `is`.
const IS: &str = "is";
/// `last`.
const LAST: &str = "last";
/// `lax`.
const LAX: &str = "lax";
/// `like_regex`.
const LIKE_REGEX: &str = "like_regex";
/// `null`.
const NULL: &str = "null";
/// `size`.
const SIZE: &str = "size";
/// `starts`.
const STARTS: &str = "starts";
/// `strict`.
const STRICT: &str = "strict";
/// `to`.
const TO: &str = "to";
/// `true`.
const TRUE: &str = "true";
/// `type`.
const TYPE: &str = "type";
/// `unknown`.
const UNKNOWN: &str = "unknown";
/// `with`.
const WITH: &str = "with";

/// [§9.46 F, Conformance Rules 2, 8 to 21] The `<JSON method>`s turborel
/// does not implement, each with the Feature that adds it.
const DEFERRED_METHODS: [(&str, &str); 13] = [
    ("bigint", "T865, \"SQL/JSON item method: bigint()\""),
    ("boolean", "T866, \"SQL/JSON item method: boolean()\""),
    ("date", "T867, \"SQL/JSON item method: date()\""),
    ("datetime", "T832, \"SQL/JSON path language: item method\""),
    ("decimal", "T868, \"SQL/JSON item method: decimal()\""),
    ("integer", "T870, \"SQL/JSON item method: integer()\""),
    ("keyvalue", "T832, \"SQL/JSON path language: item method\""),
    ("number", "T871, \"SQL/JSON item method: number()\""),
    ("string", "T872, \"SQL/JSON item method: string()\""),
    ("time", "T873, \"SQL/JSON item method: time()\""),
    ("time_tz", "T874, \"SQL/JSON item method: time_tz()\""),
    ("timestamp", "T876, \"SQL/JSON item method: timestamp()\""),
    ("timestamp_tz", "T877, \"SQL/JSON item method: timestamp_tz()\""),
];

/// The message's end that names character `at` of the path expression,
/// counted from 0, as the character it is counted from 1.
fn place(at: usize) -> String {
    format!("(at character {})", at.saturating_add(1))
}

/// The error of a path expression's text that is not of the path
/// language's tokens, `why`, at character `at`.
fn lexical(at: usize, why: &str) -> SqlError {
    SqlError::new(
        Code::Lexical,
        format!("[ISO/IEC 9075-2:2023 §9.45 SR2] the SQL/JSON path expression is not of the path language's tokens: {why} {}", place(at)),
    )
}

/// The error of a path expression that does not conform to `<JSON path
/// expression>` [§9.46 SR3], `why`, at character `at`.
fn syntax(at: usize, why: &str) -> SqlError {
    SqlError::new(
        Code::Syntax,
        format!(
            "[ISO/IEC 9075-2:2023 §9.46 F] the SQL/JSON path expression does not conform to <JSON path expression>: {why} {}",
            place(at)
        ),
    )
}

/// The error of a path expression that breaks the Syntax Rule its
/// message `cited` begins by citing, at character `at`.
fn misplaced(at: usize, cited: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("{cited} {}", place(at)))
}

/// The error of `what`, valid in a path expression but not implemented
/// (RULES.md D18).
fn unsupported(what: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] {what} is valid SQL but is not implemented (RULES.md D18)"))
}

/// The error of a path expression beyond turborel's limits, `why`
/// (RULES.md D18).
fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the SQL/JSON path expression is beyond turborel's limits: {why}"))
}
