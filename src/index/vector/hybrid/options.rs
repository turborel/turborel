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
/// The methods the vector part may be of, as the option names them.
const VECTOR_METHODS: [&str; 4] = ["HNSW", "IVF", "DISKANN", "INVERTED"];
/// [R10.2] The vector part's method by default: HNSW.
const DEFAULT_VECTOR_METHOD: &str = "HNSW";
/// The format of a locator: the format, the text part's locator's length,
/// then the text part's locator and the vector part's.
const LOCATOR_VERSION: u8 = 1;

/// The option the method declares of its own.
pub(super) fn own() -> TableOption {
    TableOption::new(
        (VECTOR_METHOD_OPTION, Kind::Choice(&VECTOR_METHODS), Value::Char(DEFAULT_VECTOR_METHOD.to_owned())),
        Rebuild::Yes,
        "The index method of the vector part: HNSW, IVF, DISKANN, or INVERTED for a SPARSE column.",
    )
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
