//! \[DESIGN\] Embedding models and vector pipelines in the catalog's
//! bytes, as `catalog/codec.rs` writes the rest of its descriptions.
//!
//! ```text
//! embeddings: models (count u32, each a model), pipelines (likewise)
//! model:      name, plug-in (str), file (u8 0, or 1 and str), blob (u8
//!             0, or 1 and its octets, u32 count and each), options
//!             (count u32, each name and value str and quoted u8),
//!             version (u8 0, or 1 and u32), dims u32, element u8 (its
//!             code), layout u8 (0 dense, 1 sparse), EXECUTE grants
//!             (count u32, each grantor str, grantee u8 0 for PUBLIC or 1
//!             and str, grantable u8)
//! pipeline:   name, source, keys (count u32, each str), text (str),
//!             chunk (str), model, target, columns (count u32, each str),
//!             asynchronous u8
//! ```

use super::{Embeddings, ModelDesc, ModelOption, PipelineDesc};
use crate::bytes::{Put, Reader};
use crate::catalog::UsageDesc;
use crate::catalog::codec::{put_name, qualified};
use crate::error::{Code, Result, SqlError};
use crate::vector::{Element, Layout};

/// The count of `n` things, as a u32.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A string, or none.
fn put_optional(out: &mut Vec<u8>, text: Option<&str>) {
    out.put_u8(u8::from(text.is_some()));
    if let Some(text) = text {
        out.put_str(text);
    }
}

/// What `put_optional` wrote.
fn optional(r: &mut Reader<'_>) -> Result<Option<String>> {
    if r.u8()? == 0 { Ok(None) } else { r.str().map(Some) }
}

/// Strings, counted.
fn put_strings(out: &mut Vec<u8>, all: &[String]) {
    out.put_u32(count(all.len()));
    for s in all {
        out.put_str(s);
    }
}

/// What `put_strings` wrote.
fn strings(r: &mut Reader<'_>) -> Result<Vec<String>> {
    (0..r.u32()?).map(|_| r.str()).collect()
}

/// `embeddings` in bytes.
pub(crate) fn put_embeddings(out: &mut Vec<u8>, embeddings: &Embeddings) {
    out.put_u32(count(embeddings.models.len()));
    embeddings.models.iter().for_each(|m| put_model(out, m));
    out.put_u32(count(embeddings.pipelines.len()));
    embeddings.pipelines.iter().for_each(|p| put_pipeline(out, p));
}

/// What `put_embeddings` wrote.
///
/// # Errors
///
/// SQLCODE -916 if it is not that.
pub(crate) fn embeddings(r: &mut Reader<'_>) -> Result<Embeddings> {
    let models = (0..r.u32()?).map(|_| model(r)).collect::<Result<_>>()?;
    let pipelines = (0..r.u32()?).map(|_| pipeline(r)).collect::<Result<_>>()?;
    Ok(Embeddings { models, pipelines })
}

fn put_model(out: &mut Vec<u8>, m: &ModelDesc) {
    put_name(out, &m.name);
    out.put_str(&m.plugin);
    put_optional(out, m.file.as_deref());
    out.put_u8(u8::from(m.blob.is_some()));
    if let Some(blob) = &m.blob {
        out.put_bytes(blob);
    }
    out.put_u32(count(m.options.len()));
    for o in &m.options {
        out.put_str(&o.name);
        out.put_str(&o.value);
        out.put_u8(u8::from(o.quoted));
    }
    out.put_u8(u8::from(m.version.is_some()));
    if let Some(version) = m.version {
        out.put_u32(version);
    }
    out.put_u32(m.dims);
    out.put_u8(m.element.code());
    out.put_u8(u8::from(m.layout == Layout::Sparse));
    out.put_u32(count(m.execute.len()));
    for u in &m.execute {
        out.put_str(&u.grantor);
        put_optional(out, u.grantee.as_deref());
        out.put_u8(u8::from(u.grantable));
    }
}

fn damaged(what: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] RULES.md D25: a model's {what} is not one there is"))
}

fn model(r: &mut Reader<'_>) -> Result<ModelDesc> {
    let (name, plugin, file) = (qualified(r)?, r.str()?, optional(r)?);
    let blob = if r.u8()? == 0 { None } else { Some(r.bytes()?.to_vec()) };
    let options =
        (0..r.u32()?).map(|_| Ok(ModelOption { name: r.str()?, value: r.str()?, quoted: r.u8()? != 0 })).collect::<Result<_>>()?;
    let version = if r.u8()? == 0 { None } else { Some(r.u32()?) };
    let dims = r.u32()?;
    let element = Element::of_code(r.u8()?).ok_or_else(|| damaged("element format"))?;
    let layout = if r.u8()? == 0 { Layout::Dense } else { Layout::Sparse };
    let execute = (0..r.u32()?)
        .map(|_| Ok(UsageDesc { grantor: r.str()?, grantee: optional(r)?, grantable: r.u8()? != 0 }))
        .collect::<Result<_>>()?;
    Ok(ModelDesc { name, plugin, file, blob, options, version, dims, element, layout, execute })
}

fn put_pipeline(out: &mut Vec<u8>, p: &PipelineDesc) {
    put_name(out, &p.name);
    put_name(out, &p.source);
    put_strings(out, &p.keys);
    out.put_str(&p.text);
    out.put_str(&p.chunk);
    put_name(out, &p.model);
    put_name(out, &p.target);
    put_strings(out, &p.columns);
    out.put_u8(u8::from(p.asynchronous));
}

fn pipeline(r: &mut Reader<'_>) -> Result<PipelineDesc> {
    let (name, source, keys, text, chunk) = (qualified(r)?, qualified(r)?, strings(r)?, r.str()?, r.str()?);
    let (model, target, columns, asynchronous) = (qualified(r)?, qualified(r)?, strings(r)?, r.u8()? != 0);
    Ok(PipelineDesc { name, source, keys, text, chunk, model, target, columns, asynchronous })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;

    /// Verifies: [EXTENSION] (vector design §12.1, §12.5) Models and
    /// pipelines read back from their bytes exactly as written, every
    /// optional part present and absent.
    #[test]
    fn embeddings_read_back_as_written() {
        let name = |auth: &str, ident: &str| QualifiedName { auth: auth.into(), ident: ident.into() };
        let option = ModelOption { name: "NGRAMS".into(), value: "2".into(), quoted: false };
        let grant = UsageDesc { grantor: "U".into(), grantee: None, grantable: true };
        let mut all = Embeddings::default();
        all.put_model(ModelDesc {
            name: name("U", "M"),
            plugin: "HASH_DENSE".into(),
            file: Some("/m.onnx".into()),
            blob: None,
            options: vec![option],
            version: Some(3),
            dims: 64,
            element: Element::Float32,
            layout: Layout::Dense,
            execute: vec![grant],
        });
        all.put_model(ModelDesc {
            name: name("U", "S"),
            plugin: "HASH_SPARSE".into(),
            file: None,
            blob: Some(vec![0, 7, 255]),
            options: Vec::new(),
            version: None,
            dims: 1 << 20,
            element: Element::Float32,
            layout: Layout::Sparse,
            execute: Vec::new(),
        });
        all.put_pipeline(PipelineDesc {
            name: name("U", "P"),
            source: name("U", "DOCS"),
            keys: vec!["ID".into()],
            text: "BODY".into(),
            chunk: "BY WORDS MAX 3 OVERLAP 0".into(),
            model: name("U", "M"),
            target: name("U", "CHUNKS"),
            columns: ["DOC", "N", "T", "E"].map(String::from).to_vec(),
            asynchronous: true,
        });
        let mut out = Vec::new();
        put_embeddings(&mut out, &all);
        let mut r = Reader::new(&out, "embeddings");
        assert_eq!(embeddings(&mut r).unwrap(), all);
        r.finish().unwrap();
    }
}
