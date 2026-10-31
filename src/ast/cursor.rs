//! Abstract syntax of a `<declare cursor>` [X3.135-1986 §8.3; ISO/IEC
//! 9075:1992 §13.1] and of a `<fetch orientation>` [§13.3]. Split from
//! ast.rs (RULES.md R-SIZE-2).

use super::ValueSpecification;

/// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] `<declare cursor>` ::=
/// DECLARE `<cursor name>` \[INSENSITIVE\] \[SCROLL\] CURSOR FOR `<cursor
/// specification>`
///
/// The `<cursor specification>` is kept as text: its `<parameter name>`s are
/// those of the procedure that OPENs it [X3.135-1986 §8.3 SR2], so it is
/// parsed and bound at OPEN [X3.135-1986 §8.8 GR3a].
#[derive(Debug, Clone)]
pub struct DeclareCursor {
    /// The `<cursor name>`.
    pub name: String,
    /// INSENSITIVE was written.
    pub insensitive: bool,
    /// SCROLL was written.
    pub scroll: bool,
    /// The text of the `<cursor specification>`.
    pub specification: String,
}

/// [ISO/IEC 9075:1992 §13.3] `<fetch orientation>` ::= NEXT | PRIOR | FIRST
/// | LAST | {ABSOLUTE | RELATIVE} `<simple value specification>`
#[derive(Debug, Clone)]
pub enum FetchOrientation {
    /// NEXT.
    Next,
    /// PRIOR.
    Prior,
    /// FIRST.
    First,
    /// LAST.
    Last,
    /// ABSOLUTE, with the row's number.
    Absolute(ValueSpecification),
    /// RELATIVE, with how far.
    Relative(ValueSpecification),
}
