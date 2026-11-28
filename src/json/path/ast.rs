//! The syntax tree of an SQL/JSON path expression [ISO/IEC 9075-2:2023
//! §9.46 F]: the mode, the `<JSON path wff>`s and the `<JSON path
//! predicate>`s of filters, as parsed.
//!
//! A production that only names another (`<JSON path wff>` is a `<JSON
//! additive expression>`, a `<JSON path predicate>` a `<JSON boolean
//! disjunction>`) has no node of its own, since its value is the other's
//! [§9.46 GR10]; nor has `<left paren> <JSON path wff> <right paren>`,
//! whose value is the wff's \[GR11f\]. Associative operators nest to the
//! left, as the Format writes them: `1 - 2 - 3` is `(1 - 2) - 3`.

use std::fmt;
use std::sync::Arc;

use crate::functions::ScalarFunction;
use crate::json::Item;
use crate::regex::Regex;

/// `<JSON path expression>`: `<JSON path mode> <JSON path wff>`.
/// \[DESIGN\] No node of `expr` is nested more than the module's
/// `MAX_PATH_NESTING` nodes deep, itself counted, so a walk of the tree may
/// recurse without bound checks of its own (RULES.md R-CF-6).
#[derive(Debug, Clone)]
pub struct Path {
    /// The `<JSON path mode>`.
    pub mode: Mode,
    /// The `<JSON path wff>`.
    pub expr: Expr,
    /// The SQL-session's time zone displacement, in minutes, as the path
    /// was parsed: what `datetime()` without a template takes a string of
    /// a datetime without time zone in, as one with a template does
    /// [§9.51 GR].
    pub zone: i16,
}

/// `<JSON path mode>`: how structural errors are handled [§9.46 GR11].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `lax`.
    Lax,
    /// `strict`.
    Strict,
}

/// `<JSON path wff>`: an expression whose value is an SQL/JSON sequence.
#[derive(Debug, Clone)]
pub enum Expr {
    /// `<JSON path literal>` [§9.45 GR1]: a `<JSON path numeric literal>`,
    /// the exact or approximate number of the `<signed numeric literal>`
    /// of its characters; a `<JSON path string literal>`, a character
    /// string, its escapes replaced; true or false, a truth value; null,
    /// the SQL/JSON null.
    Literal(Item),
    /// `<JSON path context variable>`, `$`: the context item [§9.46 GR11a].
    Context,
    /// `<JSON path named variable>`: the name it has after its `$`
    /// [§9.45 SR4b], `$a` named `a`. \[§9.46 SR4\] That a `<JSON argument>`
    /// of the PASSING clause has the name is not checked here.
    Variable(String),
    /// `<at sign>`, `@`: the current item of the innermost filter that
    /// holds it [§9.46 GR11c]; within a `<JSON path predicate>` only
    /// \[SR5\].
    Current,
    /// `<JSON last subscript>`, `last`: the last subscript of the array of
    /// the innermost `<JSON array accessor>` that holds it [§9.46 GR11d];
    /// within a `<JSON subscript>` only \[SR6\].
    Last,
    /// `<JSON accessor expression> <JSON accessor op>`: the operation
    /// applied to the accessor expression's value [§9.46 GR11g].
    Access(Box<Expr>, Accessor),
    /// `<JSON unary expression>`: `<plus sign>` or `<minus sign>` and the
    /// unary expression it applies to. It binds less tightly than an
    /// accessor op, so `-$.a` is the negation of `$.a`.
    Signed(Sign, Box<Expr>),
    /// `<JSON multiplicative expression>` or `<JSON additive expression>`:
    /// the left operand, the operator and the right operand.
    Arithmetic(Box<Expr>, Operator, Box<Expr>),
}

/// `<JSON accessor op>`.
#[derive(Debug, Clone)]
pub enum Accessor {
    /// `<JSON member accessor>`: `.` and a `<JSON path key name>` or a
    /// `<JSON path string literal>`, the key either way: the characters of
    /// the key name, the value of the string literal [§9.46 GR11g ii 1 A].
    Member(String),
    /// `<JSON wildcard member accessor>`: `.*`.
    AnyMember,
    /// `<JSON array accessor>`: `[` `<JSON subscript list>` `]`, its
    /// subscripts in order, one at least.
    Subscripts(Vec<Subscript>),
    /// `<JSON wildcard array accessor>`: `[*]`.
    AnyElement,
    /// `<JSON filter expression>`: `?(` `<JSON path predicate>` `)`.
    Filter(Box<Predicate>),
    /// `<JSON item method>`: `.` and a `<JSON method>`.
    Method(Method),
}

/// `<JSON subscript>`: `<JSON path wff 1>`, or `<JSON path wff 2> to <JSON
/// path wff 3>`.
#[derive(Debug, Clone)]
pub struct Subscript {
    /// `<JSON path wff 1>`, or `<JSON path wff 2>`: the subscript, or the
    /// first of the range.
    pub from: Expr,
    /// `<JSON path wff 3>`, the last subscript of the range, if `to` is
    /// written.
    pub to: Option<Expr>,
}

/// `<JSON method>`: an item method and the arguments written in its
/// parentheses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// `type()`.
    Type,
    /// `size()`.
    Size,
    /// `double()`.
    Double,
    /// `ceiling()`.
    Ceiling,
    /// `floor()`.
    Floor,
    /// `abs()`.
    Abs,
    /// `datetime()`, without a `<JSON datetime template>`.
    Datetime,
    /// `datetime(<JSON datetime template>)`.
    DatetimeTemplate(DatetimeTemplate),
    /// `keyvalue()`.
    Keyvalue,
    /// `bigint()`.
    Bigint,
    /// `boolean()`.
    Boolean,
    /// `date()`.
    Date,
    /// `decimal()`, `decimal(<precision>)` or `decimal(<precision>,
    /// <scale>)`.
    Decimal {
        /// The `<precision>`, if written.
        precision: Option<u32>,
        /// The `<scale>`, if written; only after a precision.
        scale: Option<u32>,
    },
    /// `integer()`.
    Integer,
    /// `number()`.
    Number,
    /// `string()`.
    String,
    /// `time()` or `time(<time precision>)`: the precision, if written.
    Time(Option<u32>),
    /// `time_tz()` or `time_tz(<time precision>)`: the precision, if
    /// written.
    TimeTz(Option<u32>),
    /// `timestamp()` or `timestamp(<timestamp precision>)`: the precision,
    /// if written.
    Timestamp(Option<u32>),
    /// `timestamp_tz()` or `timestamp_tz(<timestamp precision>)`: the
    /// precision, if written.
    TimestampTz(Option<u32>),
}

/// The `<JSON datetime template>` of `datetime()` [§9.46 SR10]: the value
/// of its string literal, and the conversion of a character string by it
/// to a value of its implicit datetime data type [§9.51], made as the path
/// is parsed. Two are equal if their values are, and one is shown as its
/// value.
#[derive(Clone)]
pub struct DatetimeTemplate {
    text: String,
    cast: Arc<dyn ScalarFunction>,
}

impl DatetimeTemplate {
    /// The template `text`, read by `cast`.
    pub(crate) fn new(text: String, cast: Arc<dyn ScalarFunction>) -> DatetimeTemplate {
        DatetimeTemplate { text, cast }
    }

    /// The value of the template's string literal.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The conversion of a character string by the template.
    pub(crate) fn cast(&self) -> &dyn ScalarFunction {
        &*self.cast
    }
}

impl fmt::Debug for DatetimeTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.text, f)
    }
}

impl PartialEq for DatetimeTemplate {
    fn eq(&self, other: &DatetimeTemplate) -> bool {
        self.text == other.text
    }
}

impl Eq for DatetimeTemplate {}

/// The sign of a `<JSON unary expression>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// `<plus sign>`.
    Plus,
    /// `<minus sign>`.
    Minus,
}

/// The operator of a `<JSON multiplicative expression>` or a `<JSON
/// additive expression>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `<asterisk>`.
    Multiply,
    /// `<solidus>`.
    Divide,
    /// `<percent>`.
    Modulo,
    /// `<plus sign>`.
    Add,
    /// `<minus sign>`.
    Subtract,
}

/// `<JSON comp op>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    /// `<double equals>`, `==`.
    Equal,
    /// `<JSON path not equals operator>`: `<not equals operator>`, `<>`,
    /// or `<alternative not equals operator>`, `!=`.
    NotEqual,
    /// `<less than operator>`, `<`.
    Less,
    /// `<greater than operator>`, `>`.
    Greater,
    /// `<less than or equals operator>`, `<=`.
    LessOrEqual,
    /// `<greater than or equals operator>`, `>=`.
    GreaterOrEqual,
}

/// `<JSON path predicate>`: a predicate of a filter, whose value is True,
/// False or Unknown [§9.46 GR12].
#[derive(Debug, Clone)]
pub enum Predicate {
    /// `<JSON exists path predicate>`: `exists (` `<JSON path wff>` `)`.
    Exists(Expr),
    /// `<JSON comparison predicate>`: the two wffs and the operator
    /// between them. Comparisons do not associate [NOTE 489].
    Compare(Expr, Comparison, Expr),
    /// `<JSON like_regex predicate>`: the wff, and its `<JSON like_regex
    /// pattern>` compiled under its `<JSON like_regex flags>`, none if not
    /// written. \[§9.46 SR7\] Both are checked when the path is parsed.
    /// Boxed, a compiled expression being several times a predicate's
    /// other forms.
    LikeRegex(Expr, Box<Regex>),
    /// `<JSON starts with predicate>`: the `<JSON starts with whole>` and
    /// the `<JSON starts with initial>`, which is an `Expr::Literal` of a
    /// string or an `Expr::Variable`. \[§9.46 SR8\] That the variable is a
    /// character string is not checked here.
    StartsWith(Expr, Expr),
    /// `<JSON unknown predicate>`: `(` `<JSON path predicate>` `) is
    /// unknown`.
    IsUnknown(Box<Predicate>),
    /// `<JSON boolean negation>`: `!` and a `<JSON delimited predicate>`.
    Not(Box<Predicate>),
    /// `<JSON boolean conjunction>`: `&&` and its two operands.
    And(Box<Predicate>, Box<Predicate>),
    /// `<JSON boolean disjunction>`: `||` and its two operands.
    Or(Box<Predicate>, Box<Predicate>),
}
