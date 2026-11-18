//! \[EXTENSION\] CREATE HYBRID VECTOR INDEX (the vector design document
//! §11.1), desugared to an index statement of the method `hybrid` and its
//! options (RULES.md D41). In the standard's notation:
//!
//! ```text
//! <create hybrid index statement> ::=
//!     CREATE HYBRID VECTOR INDEX <index name> ON <table name>
//!         ( <text column name> , <vector column name> )
//!     [ PARAMETERS ( <hybrid parameter> [ { , <hybrid parameter> }... ] )
//!     | WITH <table options> ]
//!
//! <hybrid parameter> ::=
//!       VECTOR_IDX <character string literal>
//!     | TEXT_IDX <character string literal>
//!     | MODEL <identifier> [ { . <identifier> }... ]
//!     | CHUNKER <identifier>...
//! ```
//!
//! VECTOR_IDX's string is a list of `<vector index parameter>`s by commas,
//! as CREATE VECTOR INDEX's PARAMETERS takes (`vector_index.rs`): TYPE
//! names the vector part's method, HYBRID.VECTOR_METHOD; DISTANCE its
//! METRIC; WITH TARGET ACCURACY, TARGET ACCURACY and QUANTIZATION their
//! options; each other its own, of the method's group. TEXT_IDX's is a
//! list of words and values, each an option of the group FULLTEXT, as
//! `LANGUAGE english` sets FULLTEXT.LANGUAGE. The index of §11.1 embeds
//! its text by a MODEL and a CHUNKER; this one indexes a vector column the
//! table holds, so either is refused, SQLCODE -900.

use super::vector_index::{INVERTED, method_of_type, option_of};
use crate::ast::{IndexStatement, OptionValue, SqlStatement, TableOptionValue, TableStorage};
use crate::error::{Code, Result, SqlError};
use crate::index::vector::hybrid::{HYBRID_METHOD, VECTOR_METHOD_OPTION};
use crate::index::{FULLTEXT_METHOD, HNSW_METHOD};
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// The words of the statement and its parameters.
mod words {
    pub const HYBRID: &str = "HYBRID";
    pub const VECTOR: &str = "VECTOR";
    pub const INDEX: &str = "INDEX";
    pub const PARAMETERS: &str = "PARAMETERS";
    pub const VECTOR_IDX: &str = "VECTOR_IDX";
    pub const TEXT_IDX: &str = "TEXT_IDX";
    pub const MODEL: &str = "MODEL";
    pub const CHUNKER: &str = "CHUNKER";
    pub const TYPE: &str = "TYPE";
    pub const DISTANCE: &str = "DISTANCE";
    pub const METRIC: &str = "METRIC";
    pub const QUANTIZATION: &str = "QUANTIZATION";
    pub const QUANTIZER: &str = "QUANTIZER";
    pub const TARGET_ACCURACY: &str = "TARGET_ACCURACY";
    /// The words of a target accuracy, as WITH TARGET ACCURACY and TARGET
    /// ACCURACY read.
    pub const ACCURACIES: [&str; 2] = ["WITH TARGET ACCURACY", "TARGET ACCURACY"];
}

/// What VECTOR_IDX gave: the vector part's method, and its parameters.
#[derive(Default)]
struct Vectors {
    method: Option<&'static str>,
    given: Vec<(String, OptionValue)>,
}

/// The option of VECTOR_IDX's parameter `word`, for a part of `method`.
fn vector_option(method: &str, word: &str) -> String {
    let upper = word.to_ascii_uppercase();
    let own = if upper == words::DISTANCE {
        words::METRIC
    } else if words::ACCURACIES.contains(&upper.as_str()) {
        words::TARGET_ACCURACY
    } else if upper == words::QUANTIZATION && method != INVERTED {
        words::QUANTIZER
    } else {
        word
    };
    option_of(method, own)
}

impl Parser {
    /// CREATE VECTOR INDEX, CREATE HYBRID VECTOR INDEX or CREATE INDEX,
    /// after CREATE.
    pub(super) fn create_any_index(&mut self) -> Result<SqlStatement> {
        if self.at_vector_index() {
            return self.create_vector_index();
        }
        if self.at_hybrid_index() {
            return self.create_hybrid_index();
        }
        self.create_index()
    }

    /// Whether HYBRID VECTOR INDEX follows CREATE.
    pub(super) fn at_hybrid_index(&self) -> bool {
        let word = |at: usize, w: &str| matches!(self.peek_at(at), Tok::Identifier(i) if i == w);
        self.is_word(words::HYBRID) && word(1, words::VECTOR) && word(2, words::INDEX)
    }

    /// `<create hybrid index statement>`, after CREATE.
    pub(super) fn create_hybrid_index(&mut self) -> Result<SqlStatement> {
        let method_at = self.here();
        self.advance();
        self.advance();
        self.advance();
        let named_at = self.here();
        let name = (self.identifier()?, named_at);
        self.expect_kw(kw::ON)?;
        let table = self.table_name()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let text_at = self.here();
        let text = (self.identifier()?, text_at);
        self.expect_delim(delim::COMMA)?;
        let vector_at = self.here();
        let columns = vec![text, (self.identifier()?, vector_at)];
        self.expect_delim(delim::RIGHT_PAREN)?;
        let options = if self.accept_kw(kw::WITH) {
            self.table_options()?
        } else if self.accept_word(words::PARAMETERS) {
            self.hybrid_parameters()?
        } else {
            Vec::new()
        };
        let storage = TableStorage { engine: Some((HYBRID_METHOD.to_owned(), method_at)), options };
        Ok(SqlStatement::Index(IndexStatement::Create { name, table, columns, expressions: Vec::new(), storage }))
    }

    /// ( `<hybrid parameter>` ... ), after PARAMETERS: the options they set.
    fn hybrid_parameters(&mut self) -> Result<Vec<TableOptionValue>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut options = Vec::new();
        loop {
            let at = self.here();
            if self.accept_word(words::VECTOR_IDX) {
                let text = self.character_string_literal(words::VECTOR_IDX)?;
                options.extend(vector_parameters(&text, at)?);
            } else if self.accept_word(words::TEXT_IDX) {
                let text = self.character_string_literal(words::TEXT_IDX)?;
                options.extend(text_parameters(&text, at)?);
            } else if self.is_word(words::MODEL) || self.is_word(words::CHUNKER) {
                return Err(SqlError::at(
                    Code::Unsupported,
                    at,
                    "[EXTENSION] a hybrid index indexes a vector column its table holds: it takes no MODEL or CHUNKER",
                ));
            } else {
                return Err(self.syntax("expected VECTOR_IDX, TEXT_IDX, MODEL or CHUNKER"));
            }
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(options)
    }

    /// The parameters of a VECTOR_IDX or TEXT_IDX string, read whole: each
    /// its words and value.
    fn parameter_list(&mut self) -> Result<Vec<(String, OptionValue)>> {
        let mut given = Vec::new();
        while !matches!(self.peek(), Tok::End) {
            let (word, _, value) = self.vector_index_parameter()?;
            given.push((word, value));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        if matches!(self.peek(), Tok::End) { Ok(given) } else { Err(self.syntax("expected \",\"")) }
    }
}

/// The parameters of the string `text`, read as `parameter_list` reads
/// them, every error placed at `at`, where its parameter begins.
fn parameters_of(text: &str, at: usize) -> Result<Vec<(String, OptionValue)>> {
    Parser::new(text).and_then(|mut p| p.parameter_list()).map_err(|e| SqlError { pos: Some(at), ..e })
}

/// The options VECTOR_IDX's string `text`, given at `at`, sets.
fn vector_parameters(text: &str, at: usize) -> Result<Vec<TableOptionValue>> {
    let mut vectors = Vectors::default();
    for (word, value) in parameters_of(text, at)? {
        if !word.eq_ignore_ascii_case(words::TYPE) {
            vectors.given.push((word, value));
            continue;
        }
        let named = match &value {
            OptionValue::Word(w) => method_of_type(w),
            OptionValue::Literal(_) => None,
        };
        let method =
            named.ok_or_else(|| SqlError::at(Code::SyntaxRule, at, "[EXTENSION] VECTOR_IDX's TYPE is HNSW, IVF, DISKANN or INVERTED"))?;
        vectors.method = Some(method);
    }
    let method = vectors.method.unwrap_or(HNSW_METHOD);
    let chosen =
        TableOptionValue { name: VECTOR_METHOD_OPTION.to_owned(), at, value: Some(OptionValue::Word(method.to_ascii_uppercase())) };
    let given =
        vectors.given.into_iter().map(|(word, value)| TableOptionValue { name: vector_option(method, &word), at, value: Some(value) });
    Ok(std::iter::once(chosen).chain(given).collect())
}

/// The options TEXT_IDX's string `text`, given at `at`, sets: each of the
/// group FULLTEXT.
fn text_parameters(text: &str, at: usize) -> Result<Vec<TableOptionValue>> {
    let named = |word: &str| format!("{}.{}", FULLTEXT_METHOD.to_ascii_uppercase(), word.to_ascii_uppercase().replace(' ', "_"));
    Ok(parameters_of(text, at)?.into_iter().map(|(word, value)| TableOptionValue { name: named(&word), at, value: Some(value) }).collect())
}
