//! \[EXTENSION\] Embedding models and vector pipelines as the catalog
//! describes them (the vector design's §12.1, §12.5 and §16).
//!
//! A model is an object of a schema bound to an `Embedder` plug-in its
//! host registers (`vector/embed.rs`), with the options it was made with.
//! The descriptor keeps the shape the plug-in gave as it was made, so the
//! catalog shows it while the plug-in is not registered. EXECUTE on it is
//! granted as on a routine, its schema's owner holding it grantably.
//!
//! A pipeline keeps a table of chunks and their embeddings in step with a
//! table of texts by four row triggers on the source (`db/embedding.rs`),
//! named as it is with `_INSERT`, `_DELETE`, `_UPDATE_OLD` and
//! `_UPDATE_NEW` after its name.

mod codec;

pub(crate) use codec::{embeddings, put_embeddings};

use super::{QualifiedName, UsageDesc, Who, usage_held};
use crate::vector::{Element, Layout};

/// The suffixes of a pipeline's triggers' names, in the order made.
pub const TRIGGER_SUFFIXES: [&str; 4] = ["_INSERT", "_DELETE", "_UPDATE_OLD", "_UPDATE_NEW"];

/// \[EXTENSION\] An option of a model, as written: its name in upper
/// case, its value's text, and whether that was a string literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// Its name.
    pub name: String,
    /// Its value: a string's characters, a number's digits, or a word in
    /// upper case.
    pub value: String,
    /// Whether the value was a string literal.
    pub quoted: bool,
}

/// \[EXTENSION\] An embedding model's descriptor (§12.1, §16).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDesc {
    /// Its name.
    pub name: QualifiedName,
    /// The name of the plug-in that embeds for it: its TYPE.
    pub plugin: String,
    /// FROM FILE's path, if written.
    pub file: Option<String>,
    /// FROM BLOB's octets, if given: the model's own file.
    pub blob: Option<Vec<u8>>,
    /// The options given to the plug-in, as written, by name.
    pub options: Vec<ModelOption>,
    /// Its VERSION, if given; else the plug-in's own (R12.5).
    pub version: Option<u32>,
    /// The dimensions of its vectors.
    pub dims: u32,
    /// Their element format.
    pub element: Element,
    /// Their layout.
    pub layout: Layout,
    /// EXECUTE on it: each grant.
    pub execute: Vec<UsageDesc>,
}

impl ModelDesc {
    /// Whether `who` holds EXECUTE on it, and whether grantably, its
    /// schema's owner being `owner`.
    pub fn execute_of(&self, who: Who<'_>, owner: &str) -> (bool, bool) {
        usage_held(&self.execute, who, owner)
    }

    /// The option `name`'s value, if given.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options.iter().find(|o| o.name == name).map(|o| o.value.as_str())
    }
}

/// \[EXTENSION\] A vector pipeline's descriptor (§12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineDesc {
    /// Its name.
    pub name: QualifiedName,
    /// The table of texts.
    pub source: QualifiedName,
    /// The source's key columns.
    pub keys: Vec<String>,
    /// The source's column of text.
    pub text: String,
    /// How the text is chunked, as CHUNK_TEXT reads it.
    pub chunk: String,
    /// The model.
    pub model: QualifiedName,
    /// The table of chunks.
    pub target: QualifiedName,
    /// The target's columns: one for each key, then the chunk's number,
    /// its text and its embedding, and the model's version if a fifth is
    /// named.
    pub columns: Vec<String>,
    /// MODE ASYNC: the embeddings filled after commit.
    pub asynchronous: bool,
}

impl PipelineDesc {
    /// The target's column at `at` past its keys.
    fn after_keys(&self, at: usize) -> Option<&str> {
        self.columns.get(self.keys.len().saturating_add(at)).map(String::as_str)
    }

    /// The target's column of the chunk's number.
    pub fn chunk_id_column(&self) -> Option<&str> {
        self.after_keys(0)
    }

    /// The target's column of the chunk's text.
    pub fn chunk_text_column(&self) -> Option<&str> {
        self.after_keys(1)
    }

    /// The target's column of the embedding.
    pub fn embedding_column(&self) -> Option<&str> {
        self.after_keys(2)
    }

    /// The target's column of the model's version, if it names one.
    pub fn version_column(&self) -> Option<&str> {
        self.after_keys(3)
    }

    /// The names of its triggers, in the order made.
    pub fn triggers(&self) -> Vec<QualifiedName> {
        let named = |suffix: &&str| QualifiedName { auth: self.name.auth.clone(), ident: format!("{}{suffix}", self.name.ident) };
        TRIGGER_SUFFIXES.iter().map(named).collect()
    }
}

/// \[EXTENSION\] The embedding models and vector pipelines of a catalog,
/// each kind by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embeddings {
    /// The models, by name.
    pub models: Vec<ModelDesc>,
    /// The pipelines, by name.
    pub pipelines: Vec<PipelineDesc>,
}

impl Embeddings {
    /// The model `name`, if any.
    pub fn model(&self, name: &QualifiedName) -> Option<&ModelDesc> {
        self.models.iter().find(|m| m.name == *name)
    }

    /// The pipeline `name`, if any.
    pub fn pipeline(&self, name: &QualifiedName) -> Option<&PipelineDesc> {
        self.pipelines.iter().find(|p| p.name == *name)
    }

    /// `model` recorded, in place of one of its name.
    pub fn put_model(&mut self, model: ModelDesc) {
        self.models.retain(|m| m.name != model.name);
        self.models.push(model);
        self.models.sort_by_key(|m| m.name.to_string());
    }

    /// `pipeline` recorded, in place of one of its name.
    pub fn put_pipeline(&mut self, pipeline: PipelineDesc) {
        self.pipelines.retain(|p| p.name != pipeline.name);
        self.pipelines.push(pipeline);
        self.pipelines.sort_by_key(|p| p.name.to_string());
    }

    /// The model `name` forgotten.
    pub fn remove_model(&mut self, name: &QualifiedName) {
        self.models.retain(|m| m.name != *name);
    }

    /// The pipeline `name` forgotten.
    pub fn remove_pipeline(&mut self, name: &QualifiedName) {
        self.pipelines.retain(|p| p.name != *name);
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty() && self.pipelines.is_empty()
    }
}
