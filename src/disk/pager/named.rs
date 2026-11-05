//! \[DESIGN\] What a durable pager file's pages name for themselves (RULES.md
//! D30, D48), so each is read as it was written while the file goes on under
//! other settings: the encodings its content passed through, by their ids in
//! its body, and the key its checksum was made with, by name in its trailer,
//! if the file's hash function is keyed. The pipeline new pages are written
//! through, and the key new checksums are made with, change while the file is
//! open (`Pager::set_pipeline`, `Pager::set_hash`), as STORAGE.ENCODINGS and
//! the checksum's key do; pages written before are read by what they name,
//! each pipeline resolved once among the encodings the file was opened with,
//! each key's function made once (`HashFunction::keyed_by`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::super::encoding::{Encoding, EncodingId, Pipeline};
use super::format::PAGE_NUMBER_BYTES;
use crate::error::{Code, Result, SqlError};
use crate::hash::{HashFunction, KEY_FIELD};
use crate::sync::lock_builtin;

/// The encodings of a durable file's pages: the pipeline pages are
/// written through now, the encodings a page may name, and the pipelines
/// of pages read, each resolved once.
pub(super) struct Named {
    current: Mutex<Arc<Pipeline>>,
    registry: Mutex<Vec<Arc<dyn Encoding>>>,
    seen: Mutex<Vec<(Vec<EncodingId>, Arc<Pipeline>)>>,
    path: PathBuf,
}

impl Named {
    /// Pages written through `current` from now on, those read named
    /// among `encodings`, of the file at `path`.
    pub(super) fn new(current: Pipeline, encodings: &[Arc<dyn Encoding>], path: &Path) -> Named {
        let mut registry = encodings.to_vec();
        merge(&mut registry, current.stages());
        let current = Arc::new(current);
        let seen = vec![(current.ids(), Arc::clone(&current))];
        Named { current: Mutex::new(current), registry: Mutex::new(registry), seen: Mutex::new(seen), path: path.to_path_buf() }
    }

    /// The pipeline pages are written through now.
    pub(super) fn current(&self) -> Arc<Pipeline> {
        Arc::clone(&lock_builtin(&self.current))
    }

    /// Pages are written through `pipeline` from now on; its encodings
    /// are among those a page may name.
    pub(super) fn set(&self, pipeline: Pipeline) {
        merge(&mut lock_builtin(&self.registry), pipeline.stages());
        let pipeline = Arc::new(pipeline);
        let mut seen = lock_builtin(&self.seen);
        seen.retain(|(ids, _)| *ids != pipeline.ids());
        seen.push((pipeline.ids(), Arc::clone(&pipeline)));
        drop(seen);
        *lock_builtin(&self.current) = pipeline;
    }

    /// `input`, of the page `context`, encoded through the current
    /// pipeline, and its ids, for the page to name.
    ///
    /// # Errors
    ///
    /// The encodings'.
    pub(super) fn encode<'a>(&self, context: u64, input: &'a [u8]) -> Result<(Vec<EncodingId>, Cow<'a, [u8]>)> {
        let current = self.current();
        Ok((current.ids(), current.encode(context, input)?))
    }

    /// `input`, of the page `context`, decoded through the pipeline `ids`
    /// name, as the page was written.
    ///
    /// # Errors
    ///
    /// The encodings'; SQLCODE -910 if an id names an encoding the file was
    /// not opened with.
    pub(super) fn decode<'a>(&self, context: u64, ids: &[EncodingId], input: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        self.pipeline_of(ids)?.decode(context, input)
    }

    /// The pipeline `ids` name, resolved once.
    fn pipeline_of(&self, ids: &[EncodingId]) -> Result<Arc<Pipeline>> {
        if let Some((_, pipeline)) = lock_builtin(&self.seen).iter().find(|(seen, _)| seen == ids) {
            return Ok(Arc::clone(pipeline));
        }
        let registry = lock_builtin(&self.registry).clone();
        let pipeline = Arc::new(Pipeline::resolve(ids, &registry, &self.path)?);
        lock_builtin(&self.seen).push((ids.to_vec(), Arc::clone(&pipeline)));
        Ok(pipeline)
    }
}

/// Adds to `registry` each of `stages` whose id is not there.
fn merge(registry: &mut Vec<Arc<dyn Encoding>>, stages: &[Arc<dyn Encoding>]) {
    for stage in stages {
        if !registry.iter().any(|known| known.id() == stage.id()) {
            registry.push(Arc::clone(stage));
        }
    }
}

/// The hash function a file's pages are checksummed with: the one
/// checksums are made with now, and, of a keyed function, those keyed by
/// the names pages read name, each made once.
pub(super) struct Keyed {
    current: Mutex<Arc<dyn HashFunction>>,
    others: Mutex<HashMap<String, Arc<dyn HashFunction>>>,
}

impl Keyed {
    /// Checksums made with `hash` from now on.
    pub(super) fn new(hash: Arc<dyn HashFunction>) -> Keyed {
        Keyed { current: Mutex::new(hash), others: Mutex::new(HashMap::new()) }
    }

    /// The function checksums are made with now.
    pub(super) fn current(&self) -> Arc<dyn HashFunction> {
        Arc::clone(&lock_builtin(&self.current))
    }

    /// Checksums are made with `hash` from now on: the file's function,
    /// keyed by another key.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `hash` is not the file's function, or is not keyed.
    pub(super) fn set(&self, hash: Arc<dyn HashFunction>) -> Result<()> {
        let current = self.current();
        if hash.id() != current.id() || !hash.keyed() {
            return Err(SqlError::new(
                Code::Api,
                format!(
                    "[DESIGN] a file's checksums are keyed again only by its own function, {}, keyed: not {}",
                    current.name(),
                    hash.name()
                ),
            ));
        }
        *lock_builtin(&self.current) = hash;
        Ok(())
    }

    /// The function that verifies a digest made with the key `name`: of a
    /// function not keyed, itself, `name` empty.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `name` names a key the function cannot take.
    pub(super) fn verifier(&self, name: &str) -> Result<Arc<dyn HashFunction>> {
        let current = self.current();
        if !current.keyed() || current.key_name() == Some(name) {
            return Ok(current);
        }
        if let Some(other) = lock_builtin(&self.others).get(name) {
            return Ok(Arc::clone(other));
        }
        let other = current.keyed_by(name)?;
        lock_builtin(&self.others).insert(name.to_owned(), Arc::clone(&other));
        Ok(other)
    }

    /// Bytes of a page's trailer: its number, the key's name if the
    /// function is keyed, and the digest.
    pub(super) fn trailer_len(&self) -> usize {
        let current = self.current();
        let key = if current.keyed() { KEY_FIELD } else { 0 };
        PAGE_NUMBER_BYTES.saturating_add(key).saturating_add(current.digest_len())
    }
}

#[cfg(test)]
pub(super) mod tests;
