//! \[EXTENSION\] ALTER INDEX of a hybrid index that embeds its text (the
//! vector design document §10.6, §11.1). Its MODEL, CHUNKER and MODE, and
//! its vector part's method, are how its chunks are made, so a change of
//! one is a rebuild, as REBUILD PARAMETERS is: the index is set BUILDING
//! with them, what it keeps beside it is dropped and made again of them,
//! its text embedded again, and it is set READY. A model or chunking that
//! could not be made is refused before anything changes. Any other option
//! changes as ALTER INDEX changes it, a vector part's option of the index
//! of the chunks' embeddings too. REBUILD builds that index again too.
//!
//! An index of a text and a vector column takes no MODEL, CHUNKER or MODE,
//! nor loses its MODEL, as its key would change: it is dropped and made
//! again for that. No index's HYBRID.STATE is set but by the index.

use super::super::indexes::IndexSpec;
use super::super::{Session, State};
use super::hybrid_embedded::{kept, text_of};
use super::refused;
use crate::catalog::IndexDesc;
use crate::db::embedding::model_named;
use crate::error::Result;
use crate::index::vector::hybrid::{
    CHUNKER_OPTION, HYBRID_METHOD, MODE_OPTION, MODEL_OPTION, STATE_OPTION, STATES, VECTOR_METHOD_OPTION, embeds, vector_part,
};
use crate::value::Value;
use crate::vector::chunk::Spec;

/// The options of an index that embeds its text that say how its chunks
/// are made.
const SHAPING: [&str; 4] = [MODEL_OPTION, CHUNKER_OPTION, MODE_OPTION, VECTOR_METHOD_OPTION];

/// Whether `given` names an option of `names`.
fn names(given: &[(String, Option<Value>)], names: &[&str]) -> bool {
    given.iter().any(|(n, _)| names.contains(&n.as_str()))
}

/// Whether `given` sets an option of `names` to a value that is not empty.
fn sets(given: &[(String, Option<Value>)], names: &[&str]) -> bool {
    let set = |v: &Value| if let Value::Char(s) = v { !s.trim().is_empty() } else { true };
    given.iter().any(|(n, v)| names.contains(&n.as_str()) && v.as_ref().is_some_and(set))
}

impl Session {
    /// ALTER INDEX SET, or REBUILD if `rebuilt`, of `ix`, its options
    /// `given`, on `st`.
    ///
    /// # Errors
    ///
    /// Those of `Database::altered_index`; SQLCODE -103 if HYBRID.STATE is
    /// given, or an index of two columns is given a MODEL, CHUNKER or
    /// MODE, or one of a MODEL loses it, and those of `checked`.
    pub(super) fn index_altered(&self, st: &State, ix: &IndexDesc, given: &[(String, Option<Value>)], rebuilt: bool) -> Result<()> {
        if ix.method != HYBRID_METHOD {
            return self.db.altered_index(&ix.name, given, rebuilt);
        }
        if names(given, &[STATE_OPTION]) {
            return Err(refused("a hybrid index sets its HYBRID.STATE itself"));
        }
        if !embeds(&ix.method, &ix.options) {
            if sets(given, &[MODEL_OPTION, CHUNKER_OPTION, MODE_OPTION]) {
                return Err(refused("a hybrid index of a text and a vector column takes no MODEL, CHUNKER or MODE: make it again"));
            }
            return self.db.altered_index(&ix.name, given, rebuilt);
        }
        if names(given, &[MODEL_OPTION]) && !sets(given, &[MODEL_OPTION]) {
            return Err(refused("a hybrid index of a MODEL keeps one: make it again of a text and a vector column"));
        }
        if names(given, &SHAPING) {
            return self.embedded_remade(st, ix, given);
        }
        self.db.altered_index(&ix.name, given, rebuilt)?;
        let (_, _, vectors) = kept(&ix.name, &ix.table.auth);
        let (method, _) = vector_part(ix);
        let group = format!("{}.", method.to_ascii_uppercase());
        let part: Vec<(String, Option<Value>)> = given.iter().filter(|(n, _)| n.starts_with(&group)).cloned().collect();
        if rebuilt || !part.is_empty() {
            self.db.altered_index(&vectors, &part, rebuilt)?;
        }
        Ok(())
    }

    /// The index `ix` of a MODEL, its options `given` set, made again as
    /// the module says.
    fn embedded_remade(&self, st: &State, ix: &IndexDesc, given: &[(String, Option<Value>)]) -> Result<()> {
        let mut building = given.to_vec();
        building.push((STATE_OPTION.to_owned(), Some(Value::Char(STATES[1].into()))));
        let spec = self.checked(st, ix, &building)?;
        self.db.noted(self.db.altered_index(&ix.name, &building, false))?;
        let made = self
            .kept_dropped(&ix.name, &ix.table.auth)
            .and_then(|()| self.embedding(st, &spec))
            .and_then(|of| self.kept_made(&spec, &of))
            .and_then(|()| self.state_set(&ix.name, None));
        if made.is_err() {
            let _ = self.kept_undone(&ix.name, &ix.table.auth);
        }
        made
    }

    /// The index `ix` would be, its options `given` set: refused if its
    /// MODEL is no model there, or its CHUNKER no chunking.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the model is not there; -301 if the chunking is
    /// not one.
    fn checked(&self, st: &State, ix: &IndexDesc, given: &[(String, Option<Value>)]) -> Result<IndexSpec> {
        let mut options: Vec<(String, Value)> = ix.options.iter().filter(|(n, _)| !names(given, &[n.as_str()])).cloned().collect();
        options.extend(given.iter().filter_map(|(n, v)| v.clone().map(|v| (n.clone(), v))));
        let model = text_of(&options, MODEL_OPTION).and_then(model_named).ok_or_else(|| refused("HYBRID.MODEL names no model"))?;
        let model = crate::binder::qualify(&model, &self.default_schema());
        if st.catalog.embeddings().model(&model).is_none() {
            return Err(refused(&format!("there is no embedding model {model}")));
        }
        if let Some(chunker) = text_of(&options, CHUNKER_OPTION) {
            Spec::parse(chunker)?;
        }
        let table = st.catalog.table(&ix.table).ok_or_else(|| refused(&format!("there is no table {}", ix.table)))?;
        let columns = ix.columns.iter().filter_map(|&c| table.columns.get(c)).map(|c| c.name.clone()).collect();
        Ok(IndexSpec {
            name: ix.name.clone(),
            table: ix.table.clone(),
            columns,
            method: ix.method.clone(),
            options,
            nulls: ix.nulls,
            expressions: Vec::new(),
        })
    }
}
