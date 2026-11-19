//! \[EXTENSION\] The statements of embedding models and vector pipelines
//! (the vector design's §12.1, §12.5), in the standard's notation:
//!
//! `<embedding model definition>` ::= CREATE { EMBEDDING | EXTERNAL } MODEL
//! `<model name>` \[FROM { FILE `<character string literal>` | BLOB
//! `<value specification>` }\] WITH ( `<model option>` \[{, `<model
//! option>`}...\] ), EXTERNAL MODEL the alias of §12.1 (`external`)
//!
//! `<alter embedding model statement>` ::= ALTER EMBEDDING MODEL `<model
//! name>` WITH ( `<model option>` \[{, `<model option>`}...\] )
//!
//! `<model option>` ::= `<identifier>` = { `<character string literal>` |
//! \[`<sign>`\] `<unsigned numeric literal>` | `<identifier>` }
//!
//! `<vector pipeline definition>` ::= CREATE VECTOR PIPELINE `<pipeline
//! name>` SOURCE `<table name>` ( `<column name list>` ) \[CHUNK BY
//! `<chunk unit>` MAX `<unsigned integer>` \[OVERLAP `<unsigned
//! integer>`\] \[SPLIT_ON `<character string literal>`\] \[NORMALIZE
//! WHITESPACE\]\] EMBED USING `<model name>` TARGET `<table name>` (
//! `<column name list>` ) \[MODE { SYNC | ASYNC }\]
//!
//! `<drop embedding model statement>` ::= DROP EMBEDDING MODEL `<model
//! name>` \[`<drop behavior>`\], and DROP VECTOR PIPELINE `<pipeline name>`
//! \[`<drop behavior>`\], RESTRICT if none is written.
//!
//! EMBEDDING, MODEL, VECTOR, PIPELINE and the words of the clauses are
//! `<non-reserved word>`s, identifiers elsewhere.

use super::Parser;
use crate::ast::{
    DropBehavior, DropObject, ModelDefinition, PipelineDefinition, SchemaStatement, SqlStatement, VectorObject, VectorStatement,
};
use crate::catalog::ModelOption;
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::vector::chunk::Spec;

/// The word before MODEL.
const EMBEDDING: &str = "EMBEDDING";
/// The word of a model.
const MODEL: &str = "MODEL";
/// The word before PIPELINE.
const VECTOR: &str = "VECTOR";
/// The word of a pipeline.
const PIPELINE: &str = "PIPELINE";
/// The word of FROM FILE.
const FILE: &str = "FILE";
/// The word after FROM of a model's octets.
const BLOB: &str = "BLOB";
/// The word of the alias CREATE, ALTER and DROP EXTERNAL MODEL.
const EXTERNAL: &str = "EXTERNAL";
/// An external model's option of its endpoint, ENDPOINT natively.
const LOCATION: &str = "LOCATION";
/// The native option LOCATION is.
const ENDPOINT: &str = "ENDPOINT";
/// An external model's option of its kind, which is EMBEDDINGS.
const MODEL_TYPE: &str = "MODEL_TYPE";
/// The one kind of external model.
const EMBEDDINGS: &str = "EMBEDDINGS";
/// The plug-in an external model is of, unless TYPE names another.
const REMOTE: &str = "REMOTE";
/// The pipeline's clauses.
const SOURCE: &str = "SOURCE";
const CHUNK: &str = "CHUNK";
const EMBED: &str = "EMBED";
const TARGET: &str = "TARGET";
const MODE: &str = "MODE";
const SYNC: &str = "SYNC";
const ASYNC: &str = "ASYNC";

impl Parser {
    /// Whether the word `word` is the token `n` ahead.
    fn word_at(&self, n: usize, word: &str) -> bool {
        match self.peek_at(n) {
            Tok::Identifier(s) => s == word,
            Tok::KeyWord(k) => *k == word,
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => false,
        }
    }

    /// A statement that begins CREATE, ALTER or DROP: of an embedding
    /// model or a vector pipeline, else as `schema_manipulation` reads it.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(super) fn manipulation(&mut self) -> Result<SqlStatement> {
        if self.at_vector_object() {
            return self.vector_object_statement();
        }
        if self.at_extension_statement() {
            return self.extension_statement();
        }
        self.schema_manipulation()
    }

    /// Whether a statement of an embedding model or a vector pipeline
    /// begins here: CREATE, ALTER or DROP EMBEDDING MODEL, or EXTERNAL
    /// MODEL, or CREATE or DROP VECTOR PIPELINE.
    pub(super) fn at_vector_object(&self) -> bool {
        let model = (self.word_at(1, EMBEDDING) || self.word_at(1, EXTERNAL)) && self.word_at(2, MODEL);
        let pipeline = self.word_at(1, VECTOR) && self.word_at(2, PIPELINE);
        let made = self.is_kw(kw::CREATE) || self.is_kw(kw::DROP);
        (model && (made || self.is_kw(kw::ALTER))) || (pipeline && made)
    }

    /// Takes EMBEDDING MODEL, as an object of GRANT or REVOKE, if next.
    pub(super) fn accept_model_object(&mut self) -> bool {
        let next = self.word_at(0, EMBEDDING) && self.word_at(1, MODEL);
        if next {
            self.advance();
            self.advance();
        }
        next
    }

    /// A statement of an embedding model or a vector pipeline
    /// (`at_vector_object`).
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(super) fn vector_object_statement(&mut self) -> Result<SqlStatement> {
        let start = self.here();
        let (create, alter) = (self.is_kw(kw::CREATE), self.is_kw(kw::ALTER));
        self.advance();
        // VECTOR PIPELINE, or EMBEDDING or EXTERNAL MODEL.
        let (pipeline, external) = (self.is_word(VECTOR), self.word_at(0, EXTERNAL));
        self.advance();
        self.advance();
        if !create && !alter {
            let object = if pipeline { VectorObject::Pipeline } else { VectorObject::Model };
            let name = self.table_name()?;
            let behavior = if self.is_kw(kw::CASCADE) || self.is_kw(kw::RESTRICT) { self.drop_behavior()? } else { DropBehavior::Restrict };
            return Ok(SqlStatement::Drop { object: DropObject::Vector(object, name), behavior });
        }
        let statement = if pipeline {
            VectorStatement::Pipeline(Box::new(self.pipeline_definition()?))
        } else if alter {
            let d = self.model_definition(false)?;
            VectorStatement::AlterModel(if external { self.external(d, false)? } else { d })
        } else {
            let d = self.model_definition(true)?;
            VectorStatement::Model(if external { self.external(d, true)? } else { d })
        };
        Ok(SqlStatement::Schema { statement: SchemaStatement::Vector(Box::new(statement)), text: self.text_from(start) })
    }

    /// A model's name, FROM FILE or FROM BLOB if `file` allows it, and its
    /// options.
    fn model_definition(&mut self, file: bool) -> Result<ModelDefinition> {
        let name = self.table_name()?;
        let (mut path, mut blob) = (None, None);
        if file && self.accept_kw(kw::FROM) {
            if self.is_word(BLOB) {
                self.advance();
                blob = Some(self.value_specification()?);
            } else {
                self.expect_word(FILE)?;
                let Tok::CharacterString(p) = self.peek().clone() else { return Err(self.syntax("expected the file's path as a string")) };
                self.advance();
                path = Some(p);
            }
        }
        self.expect_kw(kw::WITH)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut options = vec![self.model_option()?];
        while self.accept_delim(delim::COMMA) {
            options.push(self.model_option()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ModelDefinition { name, file: path, blob, options })
    }

    /// \[EXTENSION\] Of `<external model definition>` ::= CREATE EXTERNAL
    /// MODEL `<model name>` WITH ( LOCATION = ..., API_FORMAT = ...,
    /// MODEL_TYPE = EMBEDDINGS, MODEL = ..., CREDENTIAL = ... ), the
    /// compatibility alias of the vector design's §12.1, and its ALTER:
    /// `d` as an embedding model of the plug-in REMOTE, if
    /// `made` and no TYPE is written, its LOCATION the option ENDPOINT,
    /// MODEL_TYPE, which is EMBEDDINGS, taken away.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if MODEL_TYPE is written other than EMBEDDINGS.
    fn external(&self, mut d: ModelDefinition, made: bool) -> Result<ModelDefinition> {
        if let Some(o) = d.options.iter().find(|o| o.name == MODEL_TYPE && !o.value.eq_ignore_ascii_case(EMBEDDINGS)) {
            return Err(self.syntax(format!("an external model's {MODEL_TYPE} is {EMBEDDINGS}, not {}", o.value)));
        }
        d.options.retain(|o| o.name != MODEL_TYPE);
        for o in d.options.iter_mut().filter(|o| o.name == LOCATION) {
            ENDPOINT.clone_into(&mut o.name);
        }
        if made && !d.options.iter().any(|o| o.name == "TYPE") {
            d.options.push(ModelOption { name: "TYPE".to_owned(), value: REMOTE.to_owned(), quoted: true });
        }
        Ok(d)
    }

    /// A word: an identifier or a key word, as written in upper case.
    fn any_word(&mut self) -> Option<String> {
        let word = match self.peek() {
            Tok::Identifier(w) => w.clone(),
            Tok::KeyWord(k) => (*k).to_owned(),
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => return None,
        };
        self.advance();
        Some(word)
    }

    /// A `<model option>`.
    fn model_option(&mut self) -> Result<ModelOption> {
        let name = self.any_word().ok_or_else(|| self.syntax("expected a model option's name"))?;
        self.expect_delim(delim::EQUALS)?;
        let negative = self.accept_delim(delim::MINUS);
        let literal = match self.peek().clone() {
            Tok::CharacterString(s) => Some((s, true)),
            Tok::ExactNumeric(n) | Tok::ApproximateNumeric(n) => Some((n, false)),
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => None,
        };
        let value = match (literal, negative) {
            (Some((_, true)), true) | (None, true) => return Err(self.syntax("expected a number after the sign")),
            (Some((text, quoted)), negative) => {
                self.advance();
                (if negative { format!("-{text}") } else { text }, quoted)
            }
            (None, false) => (self.any_word().ok_or_else(|| self.syntax("expected a model option's value"))?, false),
        };
        Ok(ModelOption { name, value: value.0, quoted: value.1 })
    }

    /// A `<vector pipeline definition>`, after CREATE VECTOR PIPELINE.
    fn pipeline_definition(&mut self) -> Result<PipelineDefinition> {
        let name = self.table_name()?;
        self.expect_word(SOURCE)?;
        let source = self.table_name()?;
        let mut keys = self.column_list()?;
        let text = keys
            .pop()
            .filter(|_| !keys.is_empty())
            .ok_or_else(|| self.rule("[EXTENSION] a pipeline's source names its keys, then its text"))?;
        let mut chunk = None;
        if self.accept_word(CHUNK) {
            let start = self.here();
            while !self.word_at(0, EMBED) && !matches!(self.peek(), Tok::End) {
                self.advance();
            }
            chunk = Some(Spec::parse(&self.text_from(start))?.written());
        }
        self.expect_word(EMBED)?;
        self.expect_kw(kw::USING)?;
        let model = self.table_name()?;
        self.expect_word(TARGET)?;
        let target = self.table_name()?;
        let columns = self.column_list()?;
        let fits = columns.len() == keys.len().saturating_add(3) || columns.len() == keys.len().saturating_add(4);
        if !fits {
            return Err(self
                .rule("[EXTENSION] a pipeline's target names its keys, then chunk id, chunk text, embedding and, if any, model version"));
        }
        let mut asynchronous = false;
        if self.accept_word(MODE) {
            asynchronous = self.accept_word(ASYNC);
            if !asynchronous {
                self.expect_word(SYNC)?;
            }
        }
        Ok(PipelineDefinition { name, source, keys, text, chunk, model, target, columns, asynchronous })
    }
}
