//! \[EXTENSION\] A hybrid index's options (RULES.md D41) and locator, and
//! the descriptors of its two parts. HYBRID.VECTOR_METHOD names the
//! method of its vector part, HNSW by default (the vector design document
//! R10.2), or IVF, DISKANN or INVERTED; each other option is a full-text
//! or vector method's own, read by its part as that method reads it.

use crate::bytes::{Put, Reader};
use crate::catalog::IndexDesc;
use crate::error::Result;
use crate::index::{DISKANN_METHOD, FULLTEXT_METHOD, HNSW_METHOD, IVF_METHOD, SPARSE_METHOD};
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::Value;

/// The option of the method of the vector part.
pub const VECTOR_METHOD_OPTION: &str = "HYBRID.VECTOR_METHOD";
/// \[EXTENSION\] The option of the model an index that embeds its text
/// embeds its chunks by (§11.1, §12.1).
pub const MODEL_OPTION: &str = "HYBRID.MODEL";
/// The option of how such an index cuts its text into chunks (§12.3).
pub const CHUNKER_OPTION: &str = "HYBRID.CHUNKER";
/// The option of when such an index embeds a chunk (R12.6).
pub const MODE_OPTION: &str = "HYBRID.MODE";
/// The modes, SYNC the default.
const MODES: [&str; 2] = ["SYNC", "ASYNC"];
/// \[EXTENSION\] The option of where an index that embeds its text is in
/// making or dropping what it keeps beside it, which it sets itself.
pub const STATE_OPTION: &str = "HYBRID.STATE";
/// Its states: READY, the default, then being made and being dropped.
pub const STATES: [&str; 3] = ["READY", "BUILDING", "DROPPING"];
/// The suffixes of the names of what such an index keeps beside it, in
/// its table's schema: its table of chunks, the pipeline that fills it,
/// and the vector index of the chunks' embeddings.
pub const CHUNKS_SUFFIX: &str = "_CHUNKS";
pub const PIPELINE_SUFFIX: &str = "_PIPELINE";
pub const VECTORS_SUFFIX: &str = "_VECTORS";
/// The most characters of a model's name or a chunking.
const MOST_TEXT: usize = 1024;
/// The methods the vector part may be of, as the option names them.
const VECTOR_METHODS: [&str; 4] = ["HNSW", "IVF", "DISKANN", "INVERTED"];
/// [R10.2] The vector part's method by default: HNSW.
const DEFAULT_VECTOR_METHOD: &str = "HNSW";
/// The format of a locator: the format, the text part's locator's length,
/// then the text part's locator and the vector part's.
const LOCATOR_VERSION: u8 = 1;

/// The option the method declares of its own.
fn own() -> TableOption {
    TableOption::new(
        (VECTOR_METHOD_OPTION, Kind::Choice(&VECTOR_METHODS), Value::Char(DEFAULT_VECTOR_METHOD.to_owned())),
        Rebuild::Yes,
        "The index method of the vector part: HNSW, IVF, DISKANN, or INVERTED for a SPARSE column.",
    )
}

/// The options the method declares: its vector part's method, and those
/// of an index that embeds its text
/// (`db/index_statements/hybrid_embedded.rs`).
pub(super) fn declared() -> Vec<TableOption> {
    let text = |name: &'static str, help: &'static str| {
        TableOption::new((name, Kind::Text { max: MOST_TEXT }, Value::Char(String::new())), Rebuild::No, help)
    };
    vec![
        own(),
        text(MODEL_OPTION, "The embedding model that embeds the chunks of the text, if the index embeds it (§11.1)."),
        text(CHUNKER_OPTION, "How the text is cut into chunks, as CHUNK_TEXT reads it, if the index embeds it (§12.3)."),
        TableOption::new(
            (MODE_OPTION, Kind::Choice(&MODES), Value::Char(MODES[0].to_owned())),
            Rebuild::No,
            "SYNC to embed a chunk as it is written, ASYNC after commit, if the index embeds its text (R12.6).",
        ),
        TableOption::new(
            (STATE_OPTION, Kind::Choice(&STATES), Value::Char(STATES[0].to_owned())),
            Rebuild::No,
            "READY, or BUILDING or DROPPING while an index that embeds its text makes or drops its chunks; set by the index alone.",
        ),
    ]
}

/// \[EXTENSION\] Whether an index of `method` and `options` embeds its
/// text by a MODEL.
pub fn embeds(method: &str, options: &[(String, Value)]) -> bool {
    let named = |(name, value): &(String, Value)| {
        if let Value::Char(model) = value { name == MODEL_OPTION && !model.trim().is_empty() } else { false }
    };
    method == super::HYBRID_METHOD && options.iter().any(named)
}

/// \[EXTENSION\] Of an index that embeds its text, the method and the
/// options of the vector index of its chunks' embeddings.
pub fn vector_part(desc: &IndexDesc) -> (String, Vec<(String, Value)>) {
    let (_, vector) = parts(desc);
    (vector.method, vector.options)
}

/// The method of the vector part `desc` names.
pub(super) fn vector_method(desc: &IndexDesc) -> &'static str {
    let declared = [own()];
    let named = match option_value(&declared, &desc.options, VECTOR_METHOD_OPTION) {
        Some(Value::Char(name)) => name.as_str(),
        Some(_) | None => DEFAULT_VECTOR_METHOD,
    };
    [HNSW_METHOD, IVF_METHOD, DISKANN_METHOD, SPARSE_METHOD].into_iter().find(|m| m.eq_ignore_ascii_case(named)).unwrap_or(HNSW_METHOD)
}

/// The descriptor of the part of `desc` on its key column `at`, of
/// `method`: the options of the method's group, no nulls held, as each
/// part holds the keys of its own column alone.
fn part(desc: &IndexDesc, at: usize, method: &str) -> IndexDesc {
    let group = format!("{}.", method.to_ascii_uppercase());
    let options = desc.options.iter().filter(|(name, _)| name.starts_with(&group)).cloned().collect();
    IndexDesc {
        name: desc.name.clone(),
        table: desc.table.clone(),
        columns: desc.columns.get(at).copied().into_iter().collect(),
        method: method.to_owned(),
        unique: false,
        options,
        collations: desc.collations.get(at).cloned().into_iter().collect(),
        nulls: false,
        expressions: Vec::new(),
    }
}

/// The descriptors of the text part and of the vector part of `desc`.
pub(super) fn parts(desc: &IndexDesc) -> (IndexDesc, IndexDesc) {
    (part(desc, 0, FULLTEXT_METHOD), part(desc, 1, vector_method(desc)))
}

/// The locator of an index whose parts' locators are `text` and `vector`.
pub(super) fn locator(text: &[u8], vector: &[u8]) -> Vec<u8> {
    let mut out = vec![LOCATOR_VERSION];
    out.put_u32(u32::try_from(text.len()).unwrap_or(u32::MAX));
    out.extend_from_slice(text);
    out.extend_from_slice(vector);
    out
}

/// The parts' locators a locator holds: the text part's, the vector part's.
///
/// # Errors
///
/// SQLCODE -916 if it is not one of this format.
pub(super) fn located(locator: &[u8]) -> Result<(&[u8], &[u8])> {
    let mut reader = Reader::new(locator, "hybrid index locator");
    if reader.u8()? != LOCATOR_VERSION {
        return Err(reader.corrupt("its version is not known"));
    }
    let length = usize::try_from(reader.u32()?).map_err(|_| reader.corrupt("its text part's length is too long"))?;
    let text = reader.take(length)?;
    Ok((text, reader.rest()))
}
