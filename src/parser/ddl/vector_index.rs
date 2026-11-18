//! \[EXTENSION\] CREATE VECTOR INDEX in the vector design document's two
//! forms (§10.2, §10.3), and what ALTER INDEX adds of §10.6, desugared to
//! the index statements of RULES.md D41: a method and its options. In the
//! standard's notation:
//!
//! ```text
//! <create vector index statement> ::=
//!     CREATE VECTOR INDEX <index name> ON <table name> ( <column name> )
//!     [ USING <method name> ]
//!     { WITH <table options> | [ <vector index clause>... ] }
//!
//! <vector index clause> ::=
//!       ORGANIZATION { INMEMORY NEIGHBOR GRAPH | NEIGHBOR PARTITIONS
//!                    | DISK NEIGHBOR GRAPH | INVERTED }
//!     | DISTANCE <identifier> [ { . <identifier> }... ]
//!     | WITH TARGET ACCURACY <unsigned integer>
//!     | <vector index parameters>
//!     | QUANTIZATION { <identifier> | PQ ( <unsigned integer> [ , <unsigned integer> ] ) }
//!     | RERANK { ON | OFF }
//!     | LOCAL | GLOBAL | ONLINE | PARALLEL <unsigned integer>
//!
//! <vector index parameters> ::=
//!     PARAMETERS ( TYPE <identifier> [ { , <vector index parameter> }... ] )
//!
//! <vector index parameter> ::= <identifier>... <option value>
//!
//! <alter index maintenance> ::=
//!       REBUILD [ ONLINE ] [ PARAMETERS ( <vector index parameter> [ { , ... } ] ) ]
//!     | MERGE DELTA | LOAD | UNLOAD
//! ```
//!
//! The organizations are the methods `hnsw`, `ivf`, `diskann` and
//! `inverted`, DISK NEIGHBOR GRAPH the default. Each clause becomes an
//! option of the method's group, its name the method's in upper case:
//! DISTANCE its METRIC, TARGET ACCURACY its TARGET_ACCURACY, QUANTIZATION
//! its QUANTIZER (INVERTED's QUANTIZATION), PQ(m) a QUANTIZER of PQ and a
//! PQ_BYTES of m, RERANK its RERANK, and each parameter its own name, Oracle's
//! names for HNSW and IVF taken as theirs. In WITH, METRIC, TYPE,
//! TARGET_ACCURACY, QUANTIZATION, RERANK, MAXDOP and ONLINE are SQL
//! Server's words, a name with a period an option as it is. LOCAL and
//! GLOBAL choose nothing of a table that is not partitioned, as no table
//! is; ONLINE nothing, as every build lets writers go on (RULES.md D34);
//! PARALLEL and MAXDOP allow a build that many threads, and a build takes
//! one.

use crate::ast::{IndexMaintenance, IndexStatement, Literal, OptionValue, SqlStatement, TableName, TableOptionValue, TableStorage};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

// ---------------------------------------------------------------------------
// Methods and their organizations (the vector design document §10.1, §10.2)
// ---------------------------------------------------------------------------

/// HNSW, a graph in memory.
pub const HNSW: &str = "hnsw";
/// IVF, lists of vectors by their nearest centroid.
pub const IVF: &str = "ivf";
/// DiskANN, a graph on disk: the default organization.
pub const DISKANN: &str = "diskann";
/// The inverted index of sparse vectors.
pub const INVERTED: &str = "inverted";
/// The other name of `inverted`, as USING may give it.
pub const SPARSE: &str = "sparse";

/// The words of the clauses and of SQL Server's options.
mod words {
    pub const VECTOR: &str = "VECTOR";
    pub const ORGANIZATION: &str = "ORGANIZATION";
    pub const INMEMORY: &str = "INMEMORY";
    pub const NEIGHBOR: &str = "NEIGHBOR";
    pub const GRAPH: &str = "GRAPH";
    pub const PARTITIONS: &str = "PARTITIONS";
    pub const DISK: &str = "DISK";
    pub const INVERTED: &str = "INVERTED";
    pub const DISTANCE: &str = "DISTANCE";
    pub const TARGET: &str = "TARGET";
    pub const ACCURACY: &str = "ACCURACY";
    pub const PARAMETERS: &str = "PARAMETERS";
    pub const TYPE: &str = "TYPE";
    pub const QUANTIZATION: &str = "QUANTIZATION";
    pub const PQ: &str = "PQ";
    pub const RERANK: &str = "RERANK";
    pub const ONLINE: &str = "ONLINE";
    pub const PARALLEL: &str = "PARALLEL";
    pub const METRIC: &str = "METRIC";
    pub const TARGET_ACCURACY: &str = "TARGET_ACCURACY";
    pub const QUANTIZER: &str = "QUANTIZER";
    pub const PQ_BYTES: &str = "PQ_BYTES";
    pub const MAXDOP: &str = "MAXDOP";
    pub const REBUILD: &str = "REBUILD";
    pub const MERGE: &str = "MERGE";
    pub const DELTA: &str = "DELTA";
    pub const LOAD: &str = "LOAD";
    pub const UNLOAD: &str = "UNLOAD";
    pub const OFF: &str = "OFF";
    pub const ON: &str = "ON";
}

/// [§7] The only code width of PQ: a byte of each subspace.
const PQ_BITS: u32 = 8;

/// \[EXTENSION\] The method of the index type `word` names, in any case:
/// HNSW, IVF, DISKANN, INVERTED or SPARSE.
pub fn method_of_type(word: &str) -> Option<&'static str> {
    [HNSW, IVF, DISKANN, INVERTED]
        .into_iter()
        .find(|m| m.eq_ignore_ascii_case(word))
        .or_else(|| word.eq_ignore_ascii_case(SPARSE).then_some(INVERTED))
}

/// \[EXTENSION\] The option of `method` a parameter `word` names (§10.2):
/// Oracle's HNSW and IVF names as the methods name them, every other
/// under the method's group; a name with a period as it is.
pub fn option_of(method: &str, word: &str) -> String {
    if word.contains('.') {
        return word.to_ascii_uppercase();
    }
    let upper = word.to_ascii_uppercase();
    let own = match (method, upper.as_str()) {
        (HNSW, "NEIGHBORS") => "M",
        (HNSW, "EFCONSTRUCTION") => "EF_CONSTRUCTION",
        (HNSW, "EFSEARCH") => "EF_SEARCH",
        (IVF, "NEIGHBOR PARTITIONS" | "PARTITIONS") => "LISTS",
        (IVF, "SAMPLE_PER_PARTITION" | "SAMPLES_PER_PARTITION") => "SAMPLE_PER_LIST",
        (IVF, "NEIGHBOR PARTITION PROBES") => "PROBES",
        (INVERTED, "QUANTIZER") => "QUANTIZATION",
        (_, other) => other,
    };
    format!("{}.{own}", method.to_ascii_uppercase())
}

/// An option of a clause, before its method is known.
struct Given {
    /// Its parameter word, or a name with a period.
    word: String,
    at: usize,
    value: OptionValue,
}

/// What a CREATE VECTOR INDEX has read: its method, if named, where, and
/// its options.
#[derive(Default)]
struct Read {
    method: Option<(&'static str, usize)>,
    given: Vec<Given>,
}

impl Read {
    /// Takes the method `method`, named at `at`, refusing another.
    fn method(&mut self, parser: &Parser, method: &'static str, at: usize) -> Result<()> {
        if let Some((other, _)) = self.method.filter(|(other, _)| *other != method) {
            return Err(parser.rule(format!("[EXTENSION] the index is of one type: {other} or {method}")));
        }
        self.method = Some((method, at));
        Ok(())
    }

    fn give(&mut self, word: &str, at: usize, value: OptionValue) {
        self.given.push(Given { word: word.to_owned(), at, value });
    }

    /// The statement: its method's options of what was given.
    fn statement(self, name: (String, usize), table: TableName, columns: Vec<(String, usize)>) -> SqlStatement {
        let (method, at) = self.method.unwrap_or((DISKANN, name.1));
        let options =
            self.given.into_iter().map(|g| TableOptionValue { name: option_of(method, &g.word), at: g.at, value: Some(g.value) }).collect();
        let storage = TableStorage { engine: Some((method.to_owned(), at)), options };
        SqlStatement::Index(IndexStatement::Create { name, table, columns, storage })
    }
}

/// A word as an option's value.
fn word(w: &str) -> OptionValue {
    OptionValue::Word(w.to_ascii_uppercase())
}

impl Parser {
    /// Whether VECTOR INDEX follows CREATE.
    pub(super) fn at_vector_index(&self) -> bool {
        self.is_word(words::VECTOR) && matches!(self.peek_at(1), Tok::Identifier(w) if w == "INDEX")
    }

    /// `<create vector index statement>`, after CREATE.
    pub(super) fn create_vector_index(&mut self) -> Result<SqlStatement> {
        self.advance();
        self.advance();
        let named_at = self.here();
        let name = (self.identifier()?, named_at);
        self.expect_kw(kw::ON)?;
        let table = self.table_name()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let at = self.here();
        let columns = vec![(self.identifier()?, at)];
        self.expect_delim(delim::RIGHT_PAREN)?;
        let mut read = Read::default();
        if self.accept_kw(kw::USING) {
            let method_at = self.here();
            let given = self.identifier()?;
            let method = method_of_type(&given).ok_or_else(|| self.rule(format!("[EXTENSION] {given} is no vector index type")))?;
            read.method(self, method, method_at)?;
        }
        if self.is_kw(kw::WITH) && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::LEFT_PAREN) {
            self.advance();
            self.server_options(&mut read)?;
        } else {
            while self.vector_index_clause(&mut read)? {}
        }
        Ok(read.statement(name, table, columns))
    }

    /// A `<vector index clause>`, into `read`; false if none is next.
    fn vector_index_clause(&mut self, read: &mut Read) -> Result<bool> {
        let at = self.here();
        if self.accept_word(words::ORGANIZATION) {
            let method = self.organization()?;
            read.method(self, method, at)?;
        } else if self.accept_word(words::DISTANCE) {
            let mut name = self.identifier()?;
            while self.accept_delim(delim::PERIOD) {
                name = format!("{name}.{}", self.identifier()?);
            }
            read.give(words::METRIC, at, word(&name));
        } else if self.is_kw(kw::WITH) {
            self.advance();
            self.expect_word(words::TARGET)?;
            self.expect_word(words::ACCURACY)?;
            let n = self.unsigned_integer()?;
            read.give(words::TARGET_ACCURACY, at, OptionValue::Literal(whole(n)));
        } else if self.accept_word(words::PARAMETERS) {
            self.vector_index_parameters(read)?;
        } else if self.accept_word(words::QUANTIZATION) {
            self.quantization(read, at)?;
        } else if self.accept_word(words::RERANK) {
            let on = if self.accept_kw(kw::ON) { words::ON } else { self.expect_word(words::OFF).map(|()| words::OFF)? };
            read.give(words::RERANK, at, word(on));
        } else if self.accept_kw(kw::LOCAL) || self.accept_kw(kw::GLOBAL) || self.accept_word(words::ONLINE) {
            // Of no partitioned table, and of every build online: nothing.
        } else if self.accept_word(words::PARALLEL) {
            self.unsigned_integer()?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// The method of an ORGANIZATION.
    fn organization(&mut self) -> Result<&'static str> {
        if self.accept_word(words::INMEMORY) {
            self.expect_word(words::NEIGHBOR)?;
            self.expect_word(words::GRAPH)?;
            return Ok(HNSW);
        }
        if self.accept_word(words::DISK) {
            self.expect_word(words::NEIGHBOR)?;
            self.expect_word(words::GRAPH)?;
            return Ok(DISKANN);
        }
        if self.accept_word(words::NEIGHBOR) {
            self.expect_word(words::PARTITIONS)?;
            return Ok(IVF);
        }
        self.expect_word(words::INVERTED)?;
        Ok(INVERTED)
    }

    /// `<vector index parameters>`, after PARAMETERS.
    fn vector_index_parameters(&mut self, read: &mut Read) -> Result<()> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let at = self.here();
        self.expect_word(words::TYPE)?;
        let given = self.identifier()?;
        let method = method_of_type(&given).ok_or_else(|| self.rule(format!("[EXTENSION] {given} is no vector index type")))?;
        read.method(self, method, at)?;
        while self.accept_delim(delim::COMMA) {
            let (name, given_at, value) = self.vector_index_parameter()?;
            read.give(&name, given_at, value);
        }
        self.expect_delim(delim::RIGHT_PAREN)
    }

    /// A `<vector index parameter>`: its words, where it begins, and its
    /// value.
    fn vector_index_parameter(&mut self) -> Result<(String, usize, OptionValue)> {
        let at = self.here();
        let mut name = self.option_word()?;
        while matches!(self.peek(), Tok::Identifier(_))
            && !matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::COMMA || *d == delim::RIGHT_PAREN)
        {
            name = format!("{name} {}", self.option_word()?);
        }
        let value = self.option_value()?;
        Ok((name, at, value))
    }

    /// QUANTIZATION's value, after it.
    fn quantization(&mut self, read: &mut Read, at: usize) -> Result<()> {
        let q = self.identifier()?;
        if q == words::PQ && self.accept_delim(delim::LEFT_PAREN) {
            let m = self.unsigned_integer()?;
            if self.accept_delim(delim::COMMA) && self.unsigned_integer()? != PQ_BITS {
                return Err(self.rule("[EXTENSION] PQ codes are of 8 bits a subspace"));
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            read.give(words::PQ_BYTES, at, OptionValue::Literal(whole(m)));
        }
        read.give(words::QUANTIZER, at, word(&q));
        Ok(())
    }

    /// SQL Server's WITH ( ... ), after WITH.
    fn server_options(&mut self, read: &mut Read) -> Result<()> {
        for option in self.table_options()? {
            let Some(value) = option.value else { continue };
            let text = match &value {
                OptionValue::Word(w) => w.clone(),
                OptionValue::Literal(Literal::Character(s)) => s.clone(),
                OptionValue::Literal(_) => String::new(),
            };
            match option.name.to_ascii_uppercase().as_str() {
                words::TYPE => {
                    let method = method_of_type(&text).ok_or_else(|| self.rule(format!("[EXTENSION] {text} is no vector index type")))?;
                    read.method(self, method, option.at)?;
                }
                words::METRIC | words::QUANTIZATION => {
                    let name = if option.name.eq_ignore_ascii_case(words::METRIC) { words::METRIC } else { words::QUANTIZER };
                    read.give(name, option.at, word(&text));
                }
                words::MAXDOP | words::ONLINE => {}
                _ => read.given.push(Given { word: option.name, at: option.at, value }),
            }
        }
        Ok(())
    }

    /// The ( `<parameter>` ... ) of REBUILD PARAMETERS.
    fn rebuild_parameters(&mut self) -> Result<Vec<TableOptionValue>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut parameters = Vec::new();
        loop {
            let (name, at, value) = self.vector_index_parameter()?;
            parameters.push(TableOptionValue { name, at, value: Some(value) });
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(parameters)
    }

    /// `<alter index maintenance>`, after ALTER INDEX and its name, if one
    /// is next.
    pub(super) fn index_maintenance(&mut self) -> Result<Option<IndexMaintenance>> {
        if self.accept_word(words::REBUILD) {
            self.accept_word(words::ONLINE);
            let parameters = if self.accept_word(words::PARAMETERS) { self.rebuild_parameters()? } else { Vec::new() };
            return Ok(Some(IndexMaintenance::Rebuild(parameters)));
        }
        if self.accept_word(words::MERGE) {
            self.expect_word(words::DELTA)?;
            return Ok(Some(IndexMaintenance::MergeDelta));
        }
        if self.accept_word(words::LOAD) {
            return Ok(Some(IndexMaintenance::Load));
        }
        Ok(self.accept_word(words::UNLOAD).then_some(IndexMaintenance::Unload))
    }
}

/// The whole number `n` as a literal.
fn whole(n: u32) -> Literal {
    Literal::Exact(crate::value::Decimal::new(i128::from(n), 0))
}
