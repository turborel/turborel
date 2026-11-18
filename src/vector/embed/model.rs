//! \[EXTENSION\] A model of the catalog bound to its plug-in (§12.1): the
//! plug-in configured by the model's options, with the model's VERSION,
//! BATCH_SIZE and ON_ERROR, and the routines a call naming it is bound to,
//! its model fixed as the statement is bound (`binder/embedding.rs`):
//!
//! * EMBED(model, text \[, purpose\]), of type VECTOR of the model's shape
//!   (R12.1). It gives the values of many rows at once (`call_many`),
//!   the plug-in asked for at most BATCH_SIZE texts at a time (R12.3).
//! * EMBED_VERSION(model \[, text\]): the model's name and version, as
//!   `U.M:3`, which a column records beside its vectors (R12.5); null if
//!   the text is null.
//!
//! Texts may be embedded ahead of the calls that need them (`prime`), as
//! the queue of asynchronous embeddings does before it writes them, so a
//! statement embedding them a row at a time asks the plug-in for none.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{Embedder, ModelShape, OnError, PURPOSES, data, text_of};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::functions::ScalarFunction;
use crate::sync::lock;
use crate::value::{DataType, Value};

/// \[DESIGN\] The most texts held embedded ahead of their calls.
pub const MOST_PRIMED: usize = 4096;
/// The longest version tag: a qualified name of two identifiers, a colon
/// and a u32.
const TAG_LENGTH: u32 = 300;

/// \[EXTENSION\] A model of the catalog, bound to its plug-in.
pub struct BoundModel {
    name: QualifiedName,
    embedder: Arc<dyn Embedder>,
    version: u32,
    batch: usize,
    on_error: OnError,
    primed: Mutex<HashMap<String, Value>>,
}

impl BoundModel {
    /// The model `name` of `embedder`, configured, of `version`, asking it
    /// for at most `batch` texts at once, failing as `on_error` says.
    pub fn new(name: QualifiedName, embedder: Arc<dyn Embedder>, (version, batch, on_error): (u32, usize, OnError)) -> BoundModel {
        BoundModel { name, embedder, version, batch: batch.max(1), on_error, primed: Mutex::default() }
    }

    /// Its name.
    pub fn name(&self) -> &QualifiedName {
        &self.name
    }

    /// Its version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// What a column records of the vectors it makes: its name and version
    /// (R12.5).
    pub fn tag(&self) -> String {
        format!("{}:{}", self.name, self.version)
    }

    /// Its vectors' shape.
    pub fn shape(&self) -> ModelShape {
        self.embedder.shape()
    }

    /// Whether equal texts give equal vectors.
    pub fn deterministic(&self) -> bool {
        self.embedder.deterministic()
    }

    /// The prefix of a text embedded for the purpose `word` names.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it names none.
    pub fn prefix(&self, word: &str) -> Result<&'static str> {
        let wanted = word.trim().to_ascii_uppercase();
        let (_, purpose) = PURPOSES.iter().find(|(w, _)| *w == wanted).ok_or_else(|| data("takes 'query' or 'document' as a purpose"))?;
        Ok(self.embedder.prefix(*purpose))
    }

    /// The vectors of `texts`, in order, the plug-in asked BATCH_SIZE at a
    /// time; a text's vector null if the model failed for it and ON_ERROR
    /// is NULL.
    ///
    /// # Errors
    ///
    /// The plug-in's, past its retries, if ON_ERROR is FAIL or RETRY; -301
    /// if it gave a vector not of its shape, or not one a text.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Value>> {
        let mut out = Vec::with_capacity(texts.len());
        for part in texts.chunks(self.batch) {
            out.extend(self.embedded(part)?);
        }
        Ok(out)
    }

    /// The vectors of `texts`, at most a batch, as `embed` gives them.
    fn embedded(&self, texts: &[&str]) -> Result<Vec<Value>> {
        let tries = match self.on_error {
            OnError::Retry(more) => more.saturating_add(1),
            OnError::Fail | OnError::Null => 1,
        };
        let mut last = data("a model was never asked");
        for _ in 0..tries {
            match self.embedder.embed(texts) {
                Ok(found) => return self.checked(texts.len(), &found),
                Err(error) => last = error,
            }
        }
        match (self.on_error, texts) {
            (OnError::Null, [_]) => Ok(vec![Value::Null]),
            // Each alone, so one text the model refuses nulls only its own.
            (OnError::Null, _) => {
                texts.iter().map(|t| self.embedded(std::slice::from_ref(t)).map(|v| v.into_iter().next().unwrap_or(Value::Null))).collect()
            }
            (OnError::Fail | OnError::Retry(_), _) => Err(last),
        }
    }

    /// The values of `found`, if it is a vector of the shape for each of
    /// `count` texts.
    fn checked(&self, count: usize, found: &[crate::vector::Vector]) -> Result<Vec<Value>> {
        let shape = self.shape();
        if found.len() != count {
            return Err(data(&format!("the model {} gave {} vectors for {count} texts", self.name, found.len())));
        }
        found
            .iter()
            .map(|v| if shape.fits(v) { v.value() } else { Err(data(&format!("the model {} gave a vector not of its shape", self.name))) })
            .collect()
    }

    /// The vector of `text`: one embedded ahead, or asked of the plug-in.
    ///
    /// # Errors
    ///
    /// As `embed`; -910 if the texts embedded ahead were poisoned.
    pub fn embed_one(&self, text: &str) -> Result<Value> {
        if let Some(v) = lock(&self.primed)?.get(text) {
            return Ok(v.clone());
        }
        Ok(self.embed(&[text])?.into_iter().next().unwrap_or(Value::Null))
    }

    /// Embeds `texts` ahead of the calls that will need them, a batch at a
    /// time, past those already held, up to `MOST_PRIMED`.
    ///
    /// # Errors
    ///
    /// As `embed`.
    pub fn prime(&self, texts: &[String]) -> Result<()> {
        let held = lock(&self.primed)?.len();
        let room = MOST_PRIMED.saturating_sub(held);
        let wanted: Vec<&str> = texts.iter().map(String::as_str).take(room).collect();
        let values = self.embed(&wanted)?;
        let mut primed = lock(&self.primed)?;
        for (text, v) in wanted.into_iter().zip(values) {
            primed.insert(text.to_owned(), v);
        }
        Ok(())
    }

    /// Forgets the texts embedded ahead.
    pub fn unprime(&self) {
        if let Ok(mut primed) = lock(&self.primed) {
            primed.clear();
        }
    }
}

/// \[EXTENSION\] EMBED and VECTOR_EMBEDDING of a model bound: its
/// arguments the text and, if written, the purpose.
pub struct BoundEmbed {
    /// The name called.
    pub name: &'static str,
    /// The model.
    pub model: Arc<BoundModel>,
}

impl BoundEmbed {
    /// The text with its purpose's prefix, of the arguments `args`; none
    /// if one is null.
    fn prefixed(&self, args: &[Value]) -> Result<Option<String>> {
        let Some(text) = args.first().map(text_of).transpose()?.flatten() else { return Ok(None) };
        let prefix = match args.get(1).map(text_of).transpose()? {
            None => "",
            Some(None) => return Ok(None),
            Some(Some(word)) => self.model.prefix(&word)?,
        };
        Ok(Some(format!("{prefix}{text}")))
    }
}

impl ScalarFunction for BoundEmbed {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if !(1..=2).contains(&args.len()) || !args.iter().all(DataType::is_character) {
            return Err(super::routines::arguments(self.name, "a model, a text and, if any, 'query' or 'document'"));
        }
        self.model.shape().data_type()
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        match self.prefixed(args)? {
            Some(text) => self.model.embed_one(&text),
            None => Ok(Value::Null),
        }
    }
    fn batches(&self) -> bool {
        true
    }
    fn call_many(&self, rows: &[Vec<Value>]) -> Result<Vec<Value>> {
        let texts: Vec<Option<String>> = rows.iter().map(|r| self.prefixed(r)).collect::<Result<_>>()?;
        let given: Vec<&str> = texts.iter().flatten().map(String::as_str).collect();
        let mut embedded = self.model.embed(&given)?.into_iter();
        Ok(texts.iter().map(|t| if t.is_some() { embedded.next().unwrap_or(Value::Null) } else { Value::Null }).collect())
    }
    fn deterministic(&self) -> bool {
        self.model.deterministic()
    }
}

/// \[EXTENSION\] EMBED_VERSION of a model bound: its argument, if any, the
/// text.
pub struct BoundVersion {
    /// The model.
    pub model: Arc<BoundModel>,
}

/// The type of a version tag.
pub fn tag_type() -> DataType {
    DataType::varying(TAG_LENGTH)
}

impl ScalarFunction for BoundVersion {
    fn name(&self) -> &str {
        super::routines::EMBED_VERSION
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.len() > 1 || !args.iter().all(DataType::is_character) {
            return Err(super::routines::arguments(super::routines::EMBED_VERSION, "a model and, if any, a text"));
        }
        Ok(tag_type())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.first().is_some_and(Value::is_null) {
            return Ok(Value::Null);
        }
        Ok(Value::Char(self.model.tag()))
    }
}
