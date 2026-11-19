//! \[EXTENSION\] The table function HYBRID_SEARCH (the vector design
//! document §11.2, §11.3), in the standard's notation:
//!
//! ```text
//! <hybrid search> ::= HYBRID_SEARCH ( <hybrid argument>
//!     [ { , <hybrid argument> }... ] ) [ [ AS ] <correlation name>
//!     [ <derived column list> ] ]
//!
//! <hybrid argument> ::=
//!       INDEX = <index name>
//!     | QUERY_TEXT = <value expression>
//!     | QUERY_VECTOR = <value expression>
//!     | TOP_N = <unsigned integer>
//!     | CANDIDATES = <unsigned integer>
//!     | FUSION = { 'RRF' | 'WEIGHTED' }
//!     | VECTOR_WEIGHT = <unsigned numeric literal>
//!     | TEXT_WEIGHT = <unsigned numeric literal>
//!     | RRF_K = <unsigned integer>
//! ```
//!
//! INDEX, QUERY_TEXT and TOP_N are written, the others at most once each,
//! in any order. Of no QUERY_VECTOR, QUERY_TEXT is embedded by the
//! index's MODEL (§11.2). The binder makes it
//! the query it stands for (`binder/hybrid_search.rs`), reading each
//! value argument again there, so it keeps their text.

use crate::ast::{DerivedTable, Fusion, HybridSearch, SearchArgument, SearchValue, TableReference};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, Token, delim, lex};
use crate::parser::Parser;

/// The function's name.
const HYBRID_SEARCH: &str = "HYBRID_SEARCH";
/// The words of its arguments.
mod words {
    pub const INDEX: &str = "INDEX";
    pub const QUERY_TEXT: &str = "QUERY_TEXT";
    pub const QUERY_VECTOR: &str = "QUERY_VECTOR";
    pub const TOP_N: &str = "TOP_N";
    pub const CANDIDATES: &str = "CANDIDATES";
    pub const FUSION: &str = "FUSION";
    pub const VECTOR_WEIGHT: &str = "VECTOR_WEIGHT";
    pub const TEXT_WEIGHT: &str = "TEXT_WEIGHT";
    pub const RRF_K: &str = "RRF_K";
    pub const RRF: &str = "RRF";
    pub const WEIGHTED: &str = "WEIGHTED";
}
/// [§11.3] The K of reciprocal rank fusion by default.
const DEFAULT_RRF_K: u32 = 60;

/// The arguments read so far.
#[derive(Default)]
struct Arguments {
    index: Option<String>,
    text: Option<SearchValue>,
    vector: Option<SearchValue>,
    vector_first: bool,
    top_n: Option<u32>,
    candidates: Option<u32>,
    fusion: Option<Fusion>,
    vector_weight: Option<f64>,
    text_weight: Option<f64>,
    rrf_k: Option<u32>,
    dynamic: Option<usize>,
}

impl Parser {
    /// Whether a `<hybrid search>` begins here.
    pub(super) fn at_hybrid_search(&self) -> bool {
        self.is_word(HYBRID_SEARCH) && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::LEFT_PAREN)
    }

    /// A `<hybrid search>`.
    pub(super) fn hybrid_search(&mut self) -> Result<TableReference> {
        let at = self.here();
        self.advance();
        self.advance();
        let mut given = Arguments::default();
        loop {
            self.hybrid_argument(&mut given)?;
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let correlation = self.correlation()?.unwrap_or_else(|| HYBRID_SEARCH.to_owned());
        let columns = self.derived_columns()?;
        let missing = |what: &str| self.rule(format!("[EXTENSION] HYBRID_SEARCH names its {what}"));
        let search = HybridSearch {
            index: given.index.ok_or_else(|| missing(words::INDEX))?,
            text: given.text.ok_or_else(|| missing(words::QUERY_TEXT))?,
            vector: given.vector,
            vector_first: given.vector_first,
            top_n: given.top_n.ok_or_else(|| missing(words::TOP_N))?,
            candidates: given.candidates,
            fusion: given.fusion.unwrap_or(Fusion::Rrf),
            weights: (given.vector_weight.unwrap_or(1.0_f64), given.text_weight.unwrap_or(1.0_f64)),
            rrf_k: given.rrf_k.unwrap_or(DEFAULT_RRF_K),
            correlation,
            columns,
            dynamic: given.dynamic,
            params: self.params.clone(),
            extensions: self.extensions,
            at,
        };
        Ok(TableReference::Hybrid(Box::new(search)))
    }

    /// A `<hybrid argument>`, into `given`.
    fn hybrid_argument(&mut self, given: &mut Arguments) -> Result<()> {
        let at = self.here();
        let Tok::Identifier(word) = self.peek().clone() else {
            return Err(self.syntax("expected an argument of HYBRID_SEARCH"));
        };
        self.advance();
        self.expect_delim(delim::EQUALS)?;
        let fresh = match word.as_str() {
            words::INDEX => given.index.replace(self.identifier()?).is_none(),
            words::QUERY_TEXT => {
                let value = self.search_value(given)?;
                given.text.replace(value).is_none()
            }
            words::QUERY_VECTOR => {
                given.vector_first = given.text.is_none();
                let value = self.search_value(given)?;
                given.vector.replace(value).is_none()
            }
            words::TOP_N => given.top_n.replace(self.unsigned_integer()?).is_none(),
            words::CANDIDATES => given.candidates.replace(self.unsigned_integer()?).is_none(),
            words::RRF_K => given.rrf_k.replace(self.unsigned_integer()?).is_none(),
            words::FUSION => given.fusion.replace(self.fusion()?).is_none(),
            words::VECTOR_WEIGHT => given.vector_weight.replace(self.weight()?).is_none(),
            words::TEXT_WEIGHT => given.text_weight.replace(self.weight()?).is_none(),
            _ => {
                let expected = "expected INDEX, QUERY_TEXT, QUERY_VECTOR, TOP_N, CANDIDATES, FUSION, VECTOR_WEIGHT, TEXT_WEIGHT or RRF_K";
                return Err(SqlError::at(Code::Syntax, at, expected));
            }
        };
        if fresh { Ok(()) } else { Err(self.rule(format!("[EXTENSION] HYBRID_SEARCH's {word} is written once"))) }
    }

    /// A `<value expression>` argument, as written; the dynamic parameters
    /// before the first such noted in `given`.
    fn search_value(&mut self, given: &mut Arguments) -> Result<SearchValue> {
        if given.text.is_none() && given.vector.is_none() && self.dynamic.is_some() {
            let before = self.toks.iter().take(self.pos).filter(|t| matches!(t.tok, Tok::Delimiter(delim::QUESTION))).count();
            given.dynamic = Some(before);
        }
        let start = self.here();
        let ends = |t: &Tok| matches!(t, Tok::Delimiter(d) if *d == delim::COMMA || *d == delim::RIGHT_PAREN);
        let literal = match self.peek() {
            Tok::CharacterString(s) => ends(self.peek_at(1)).then(|| s.clone()),
            Tok::Identifier(_)
            | Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => None,
        };
        self.value_expression()?;
        Ok(SearchValue { text: self.text_from(start), literal })
    }

    /// FUSION's value: 'RRF' or 'WEIGHTED'.
    fn fusion(&mut self) -> Result<Fusion> {
        let word = match self.peek().clone() {
            Tok::CharacterString(s) | Tok::Identifier(s) => s.to_ascii_uppercase(),
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => String::new(),
        };
        let fusion = match word.as_str() {
            words::RRF => Fusion::Rrf,
            words::WEIGHTED => Fusion::Weighted,
            _ => return Err(self.syntax("expected 'RRF' or 'WEIGHTED'")),
        };
        self.advance();
        Ok(fusion)
    }

    /// A weight: an `<unsigned numeric literal>`, finite.
    fn weight(&mut self) -> Result<f64> {
        let read = match self.peek() {
            Tok::ExactNumeric(text) | Tok::ApproximateNumeric(text) => text.parse::<f64>().ok().filter(|w| w.is_finite()),
            Tok::CharacterString(_)
            | Tok::Identifier(_)
            | Tok::KeyWord(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => None,
        };
        let weight = read.ok_or_else(|| self.rule("[EXTENSION] a weight of HYBRID_SEARCH is an unsigned numeric literal"))?;
        self.advance();
        Ok(weight)
    }
}

impl Parser {
    /// \[EXTENSION\] The derived table of `text`, the query the binder
    /// writes for HYBRID_SEARCH `search`, copies of its value arguments'
    /// text in it, each of the argument `copies` names in their order:
    /// read as they were, in the statement `search` is of, each dynamic
    /// parameter of a copy numbered as it was in the argument, however
    /// many copies there are.
    ///
    /// # Errors
    ///
    /// As a `<derived table>` read.
    pub(crate) fn hybrid_expansion(text: &str, search: &HybridSearch, copies: &[SearchArgument]) -> Result<DerivedTable> {
        let mut parser = Parser::for_procedure(text, search.extensions, search.params.clone())?;
        parser.catalogs = super::super::Catalogs::Kept;
        if let Some(before) = search.dynamic {
            let earlier = Token { tok: Tok::Delimiter(delim::QUESTION), start: 0, end: 0 };
            parser.toks.splice(0..0, std::iter::repeat_n(earlier, before)).for_each(drop);
            parser.pos = before;
            parser.dynamic = Some(before);
            parser.numbers = numbered(search, before, copies)?;
        }
        let read = parser.table_primary()?;
        if !matches!(parser.peek(), Tok::End) {
            return Err(parser.syntax("expected the end of HYBRID_SEARCH's query"));
        }
        match read {
            TableReference::Derived(derived) => Ok(derived),
            TableReference::Table(_)
            | TableReference::Joined(_)
            | TableReference::Unnest(_)
            | TableReference::Recognized(_)
            | TableReference::Polymorphic(_)
            | TableReference::Json(_)
            | TableReference::Graph(_)
            | TableReference::Hybrid(_)
            | TableReference::Gql(_)
            | TableReference::Delta(_) => Err(parser.syntax("expected HYBRID_SEARCH's query")),
        }
    }
}

/// The dynamic parameters of `value`'s text.
fn dynamic_in(value: &SearchValue) -> Result<usize> {
    Ok(lex(&value.text)?.iter().filter(|t| matches!(t.tok, Tok::Delimiter(delim::QUESTION))).count())
}

/// The number of each dynamic parameter of the query of `search`, by its
/// place: the `before` of the statement before its first value argument,
/// then each copy's, of its argument's numbers. The arguments' own are
/// numbered in the order they are written in the statement.
fn numbered(search: &HybridSearch, before: usize, copies: &[SearchArgument]) -> Result<Vec<usize>> {
    let text = dynamic_in(&search.text)?;
    let vector = search.vector.as_ref().map_or(Ok(0), dynamic_in)?;
    let (text_from, vector_from) =
        if search.vector_first { (before.saturating_add(vector), before) } else { (before, before.saturating_add(text)) };
    let mut numbers: Vec<usize> = (0..before).collect();
    for copy in copies {
        let (from, count) = match copy {
            SearchArgument::Text => (text_from, text),
            SearchArgument::Vector => (vector_from, vector),
        };
        numbers.extend(from..from.saturating_add(count));
    }
    Ok(numbers)
}
