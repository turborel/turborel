//! \[EXTENSION\] The routines of §12.2 and §12.3, as scalar functions.
//!
//! * `EMBED(model, text [, 'query' | 'document'])`, and VECTOR_EMBEDDING
//!   of the same arguments: the vector of `text` by the model named, the
//!   model's prefix for the purpose put first. Its type is VECTOR, of any
//!   shape, as the model is a value when a statement is bound; R12.1's
//!   VECTOR(DIMENSIONS, FORMAT) needs the binder to read a model named by
//!   a literal. A model that fails does as its `on_error` says (R12.4).
//! * `CHUNK_TEXT(text [, spec])`, and VECTOR_CHUNKS and AI_GENERATE_CHUNKS
//!   of the same arguments: the chunks of `text` (`vector/chunk.rs`), an
//!   ARRAY of ROW(CHUNK_ID, CHUNK_OFFSET, CHUNK_LENGTH, CHUNK_TEXT), which
//!   UNNEST makes the table §12.3 names. The specification is one string,
//!   `'BY WORDS MAX 256 OVERLAP 32'`, or the parts in place, `CHUNK_TEXT(
//!   text, unit, max [, overlap [, split_on [, normalize]]])`, a unit
//!   `'TOKENS model'` naming its model. A constant specification is read
//!   once, its SPLIT_ON compiled once (`specialize`).
//!
//! A null argument gives null. A text is a character string or a
//! character large object of at most `MOST_TEXT_BYTES`.

use std::sync::Arc;

use super::{Embedder, Embedders, OnError, Purpose, any_vector};
use crate::error::{Code, Result, SqlError};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::memory::{MemoryBudget, Reservation};
use crate::value::types::{Field, MAX_ARRAY_CARDINALITY};
use crate::value::{DataType, Decimal, MAX_CHARACTER_LENGTH, Spelled, Value};
use crate::vector::chunk::{Chunk, Chunker, Span, Spec, quote};

/// The names of EMBED.
pub const EMBED_NAMES: [&str; 2] = ["EMBED", "VECTOR_EMBEDDING"];
/// \[EXTENSION\] The name of EMBED_VERSION (R12.5).
pub const EMBED_VERSION: &str = "EMBED_VERSION";
/// The names of CHUNK_TEXT.
pub const CHUNK_NAMES: [&str; 3] = ["CHUNK_TEXT", "VECTOR_CHUNKS", "AI_GENERATE_CHUNKS"];
/// \[DESIGN\] The most octets of a large object a routine reads.
pub const MOST_TEXT_BYTES: u64 = 1 << 24;
/// The purposes EMBED's third argument names.
pub(super) const PURPOSES: [(&str, Purpose); 2] = [("QUERY", Purpose::Query), ("DOCUMENT", Purpose::Document)];
/// The fields of a chunk.
const CHUNK_FIELDS: [&str; 4] = ["CHUNK_ID", "CHUNK_OFFSET", "CHUNK_LENGTH", "CHUNK_TEXT"];
/// The digits of an INTEGER.
const INTEGER_DIGITS: u32 = 10;
/// \[DESIGN\] The bytes a prepared specification is counted at, beside
/// its pattern's.
const PREPARED_BYTES: usize = 4096;
/// \[DESIGN\] The bytes a pattern's character is counted at, compiled.
const PATTERN_BYTES: usize = 64;

/// The routines of the models `models`: EMBED and CHUNK_TEXT by each of
/// their names, for `Database::register_scalar_function`.
pub fn routines(models: &Arc<Embedders>) -> Vec<Box<dyn ScalarFunction>> {
    let mut out: Vec<Box<dyn ScalarFunction>> = Vec::new();
    for name in EMBED_NAMES {
        out.push(Box::new(EmbedFunction { name, models: Arc::clone(models) }));
    }
    for name in CHUNK_NAMES {
        out.push(Box::new(ChunkFunction { name, models: Arc::clone(models) }));
    }
    out.push(Box::new(VersionFunction { models: Arc::clone(models) }));
    out
}

/// \[EXTENSION\] EMBED_VERSION of a model named by a value: the name and
/// version of the plug-in it names (R12.5). Of a model of the catalog, the
/// binder binds `BoundVersion` (`model.rs`).
struct VersionFunction {
    models: Arc<Embedders>,
}

impl ScalarFunction for VersionFunction {
    fn name(&self) -> &str {
        EMBED_VERSION
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if !(1..=2).contains(&args.len()) || !args.iter().all(character) {
            return Err(arguments(EMBED_VERSION, "a model's name and, if any, a text"));
        }
        Ok(super::tag_type())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let texts: Vec<Option<String>> = args.iter().map(text_of).collect::<Result<_>>()?;
        let (Some(Some(name)), None | Some(Some(_))) = (texts.first(), texts.get(1)) else { return Ok(Value::Null) };
        let model = self.models.named(name)?.ok_or_else(|| data(&format!("names no model {name}")))?;
        Ok(Value::Char(format!("{}:{}", model.name(), model.version())))
    }
    fn deterministic(&self) -> bool {
        false
    }
}

pub(super) fn data(detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {detail}"))
}

pub(super) fn arguments(name: &str, what: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {name} takes {what}"))
}

/// The text of a character value; none for null.
///
/// # Errors
///
/// SQLCODE -301 for a value not a character string, or a large object
/// past `MOST_TEXT_BYTES`; SQLCODE -910 if one cannot be read.
pub fn text_of(v: &Value) -> Result<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::Char(s) => Ok(Some(s.clone())),
        Value::Lob(lob) => {
            let bytes = lob.bounded(MOST_TEXT_BYTES)?.ok_or_else(|| data("reads a text of at most 16 MiB"))?;
            String::from_utf8(bytes).map(Some).map_err(|_| SqlError::new(Code::Api, "[EXTENSION] a text is not UTF-8"))
        }
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => Err(data("takes a character string where a text goes")),
    }
}

/// Whether `t` is a character string type.
fn character(t: &DataType) -> bool {
    t.is_character()
}

/// \[EXTENSION\] EMBED and VECTOR_EMBEDDING.
pub struct EmbedFunction {
    name: &'static str,
    models: Arc<Embedders>,
}

impl EmbedFunction {
    /// The model `name` names.
    fn model(&self, name: &str) -> Result<Arc<dyn Embedder>> {
        self.models.named(name)?.ok_or_else(|| data(&format!("names no model {name}")))
    }

    /// The vector of `text` by `model`, as its `on_error` says.
    fn embedded(model: &dyn Embedder, text: &str) -> Result<Value> {
        let tries = match model.on_error() {
            OnError::Retry(more) => more.saturating_add(1),
            OnError::Fail | OnError::Null => 1,
        };
        let mut last = data("a model was never asked");
        for _ in 0..tries {
            match model.embed(&[text]) {
                Ok(found) => return Self::checked(model, &found),
                Err(error) => last = error,
            }
        }
        match model.on_error() {
            OnError::Null => Ok(Value::Null),
            OnError::Fail | OnError::Retry(_) => Err(last),
        }
    }

    /// The one vector `found` holds, if it has the model's shape.
    fn checked(model: &dyn Embedder, found: &[crate::vector::Vector]) -> Result<Value> {
        let shape = model.shape();
        let [v] = found else { return Err(data(&format!("the model {} gave not one vector a text", model.name()))) };
        if shape.fits(v) { v.value() } else { Err(data(&format!("the model {} gave a vector not of its shape", model.name()))) }
    }
}

impl ScalarFunction for EmbedFunction {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if !(2..=3).contains(&args.len()) || !args.iter().all(character) {
            return Err(arguments(self.name, "a model's name, a text and, if any, 'query' or 'document'"));
        }
        any_vector()
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let texts: Vec<Option<String>> = args.iter().map(text_of).collect::<Result<_>>()?;
        let (Some(Some(name)), Some(Some(text))) = (texts.first(), texts.get(1)) else { return Ok(Value::Null) };
        let model = self.model(name)?;
        let prefix = match texts.get(2) {
            Some(Some(word)) => {
                let wanted = word.trim().to_ascii_uppercase();
                let (_, purpose) =
                    PURPOSES.iter().find(|(w, _)| *w == wanted).ok_or_else(|| data("takes 'query' or 'document' as a purpose"))?;
                model.prefix(*purpose).to_owned()
            }
            Some(None) => return Ok(Value::Null),
            None => String::new(),
        };
        Self::embedded(model.as_ref(), &format!("{prefix}{text}"))
    }
    fn deterministic(&self) -> bool {
        self.models.deterministic()
    }
}

/// \[EXTENSION\] CHUNK_TEXT, VECTOR_CHUNKS and AI_GENERATE_CHUNKS.
pub struct ChunkFunction {
    name: &'static str,
    models: Arc<Embedders>,
}

/// The type of an INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: INTEGER_DIGITS, scale: 0, spelled: Spelled::Integer }
}

/// The type of CHUNK_TEXT's result.
fn chunks_type() -> DataType {
    let types = [integer(), integer(), integer(), DataType::varying(MAX_CHARACTER_LENGTH)];
    let fields =
        CHUNK_FIELDS.iter().zip(types).map(|(name, data_type)| Field { name: (*name).to_owned(), data_type, unnamed: false }).collect();
    DataType::array(DataType::row(fields), MAX_ARRAY_CARDINALITY)
}

/// The count `v` gives; none for null.
fn count_of(v: &Value) -> Result<Option<usize>> {
    let Value::Exact(Decimal { units, scale: 0 }) = v else {
        return if v.is_null() { Ok(None) } else { Err(data("takes a whole number where a count goes")) };
    };
    usize::try_from(*units).map(Some).map_err(|_| data("takes a count of 0 or more"))
}

/// The specification CHUNK_TEXT's arguments after the text give; none if
/// one is null. Each part in place is written out and read as a string.
fn spec_of(args: &[Option<&Value>]) -> Result<Option<Spec>> {
    let written = match args {
        [] => return Ok(Some(Spec::default())),
        [Some(spec)] => return text_of(spec)?.map(|s| Spec::parse(&s)).transpose(),
        [Some(unit), Some(max), rest @ ..] => {
            let (Some(unit), Some(max)) = (text_of(unit)?, count_of(max)?) else { return Ok(None) };
            let overlap = match rest.first() {
                Some(Some(overlap)) => count_of(overlap)?,
                Some(None) | None => Some(0),
            };
            let Some(overlap) = overlap else { return Ok(None) };
            // A null SPLIT_ON cuts nowhere.
            let split = match rest.get(1) {
                Some(Some(split)) => text_of(split)?.map(|p| format!(" SPLIT_ON {}", quote(&p))).unwrap_or_default(),
                Some(None) | None => String::new(),
            };
            let normalize = matches!(rest.get(2), Some(Some(Value::Bool(true))));
            let normalized = if normalize { " NORMALIZE WHITESPACE" } else { "" };
            format!("BY {unit} MAX {max} OVERLAP {overlap}{split}{normalized}")
        }
        _ => return Ok(None),
    };
    Spec::parse(&written).map(Some)
}

/// The value of `chunks`.
fn chunks_value(chunks: Vec<Chunk>) -> Result<Value> {
    let most = usize::try_from(MAX_ARRAY_CARDINALITY).unwrap_or(usize::MAX);
    if chunks.len() > most {
        return Err(data("gives at most 65535 chunks of a text"));
    }
    let longest = usize::try_from(MAX_CHARACTER_LENGTH).unwrap_or(usize::MAX);
    let whole = |n: usize| Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0));
    let mut rows = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if chunk.text.chars().count() > longest {
            return Err(data("gives chunks of at most 32767 characters"));
        }
        let fields: Arc<[Value]> = Arc::from([whole(chunk.id), whole(chunk.offset), whole(chunk.length), Value::Char(chunk.text)]);
        rows.push(Value::Row(fields));
    }
    Ok(Value::Array(Arc::from(rows)))
}

impl ChunkFunction {
    /// The model whose tokens `chunker` counts, if it counts tokens.
    fn tokenizer(&self, chunker: &Chunker) -> Result<Option<Arc<dyn Embedder>>> {
        let Some(name) = chunker.spec().model.as_deref() else { return Ok(None) };
        self.models.named(name)?.map(Some).ok_or_else(|| data(&format!("names no model {name}")))
    }
}

/// The chunks of `text` by `chunker`, tokens by `model`.
fn chunked(chunker: &Chunker, model: Option<&dyn Embedder>, text: &str) -> Result<Value> {
    let words = |t: &str| crate::vector::chunk::words_of(&t.chars().collect::<Vec<_>>());
    let tokens = |t: &str| -> Vec<Span> { model.map_or_else(|| words(t), |m| m.tokens(t)) };
    chunks_value(chunker.chunks(text, &tokens)?)
}

impl ScalarFunction for ChunkFunction {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let fits = match args {
            [text] | [text, _] => character(text) && args.iter().all(character),
            [text, unit, max, rest @ ..] => {
                let numbers = std::iter::once(max).chain(rest.first()).all(DataType::is_numeric);
                rest.len() <= 3 && character(text) && character(unit) && numbers
            }
            [] => false,
        };
        if fits {
            Ok(chunks_type())
        } else {
            Err(arguments(self.name, "a text and a specification, or a unit, MAX, OVERLAP, SPLIT_ON and NORMALIZE"))
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some((first, rest)) = args.split_first() else { return Err(arguments(self.name, "a text")) };
        let given: Vec<Option<&Value>> = rest.iter().map(Some).collect();
        let (Some(text), Some(spec)) = (text_of(first)?, spec_of(&given)?) else { return Ok(Value::Null) };
        let chunker = spec.compiled()?;
        let model = self.tokenizer(&chunker)?;
        chunked(&chunker, model.as_deref(), &text)
    }
    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
        let rest = args.get(1..)?;
        if rest.iter().any(Option::is_none) {
            return None;
        }
        let chunker = spec_of(rest).ok()??.compiled().ok()?;
        let model = self.tokenizer(&chunker).ok()?;
        let pattern = chunker.spec().split_on.as_ref().map_or(0, String::len);
        let bytes = PREPARED_BYTES.saturating_add(pattern.saturating_mul(PATTERN_BYTES));
        let held = budget.reserve(bytes, "a chunking specification prepared").ok()?;
        Some(ScalarRef(Arc::new(PreparedChunk { name: self.name, chunker, model, _held: held })))
    }
}

/// CHUNK_TEXT of a constant specification, read once.
struct PreparedChunk {
    name: &'static str,
    chunker: Chunker,
    model: Option<Arc<dyn Embedder>>,
    _held: Reservation,
}

impl ScalarFunction for PreparedChunk {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        ChunkFunction { name: self.name, models: Arc::new(Embedders::new()) }.return_type(args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(text) = args.first().map(text_of).transpose()?.flatten() else { return Ok(Value::Null) };
        chunked(&self.chunker, self.model.as_deref(), &text)
    }
}
